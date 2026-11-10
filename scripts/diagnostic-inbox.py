#!/usr/bin/env python3
"""List and pull ClipClub diagnostic reports without putting keys in arguments."""
import argparse
import getpass
import http.client
import ipaddress
import json
import os
from pathlib import Path
import re
import stat
import sys
import tempfile
import urllib.error
import urllib.parse
import urllib.request

PROJECT = Path(__file__).resolve().parent.parent
DEFAULT_CONFIG = PROJECT / '.build' / 'diagnostic-inbox.json'
DEFAULT_OUTPUT = PROJECT / '.build' / 'diagnostics'
REPORT_ID = re.compile(r'[0-9a-f]{32}\Z')
MAX_RESPONSE = 1024 * 1024
USER_AGENT = 'ClipClub/0.1.3 (diagnostic inbox client)'


class InboxError(Exception):
    pass


class NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, req, fp, code, msg, headers, newurl):
        return None


def endpoint_url(value):
    try:
        url = urllib.parse.urlsplit(value.strip())
        loopback = url.hostname == 'localhost'
        if url.hostname:
            try:
                loopback = ipaddress.ip_address(url.hostname).is_loopback
            except ValueError:
                pass
        if (not url.hostname or url.username or url.password or url.query or url.fragment or
                url.path not in ('', '/') or
                (url.scheme != 'https' and not (url.scheme == 'http' and loopback))):
            raise ValueError()
        url.port  # Validate port without resolving the endpoint.
    except (ValueError, AttributeError):
        raise InboxError('Use an HTTPS inbox origin, such as https://inbox.example.com.') from None
    return urllib.parse.urlunsplit((url.scheme, url.netloc, '', '', ''))


def read_config(path):
    if not path.exists():
        return {}
    if path.is_symlink() or not path.is_file():
        raise InboxError('The inbox config must be a regular private file.')
    if os.name == 'posix' and stat.S_IMODE(path.stat().st_mode) & 0o077:
        raise InboxError('The inbox config is readable by others. Run chmod 600 on it.')
    try:
        data = json.loads(path.read_text(encoding='utf-8'))
    except (ValueError, OSError):
        raise InboxError('Could not read the inbox config.') from None
    if not isinstance(data, dict):
        raise InboxError('Invalid inbox config.')
    return data


def private_directory(path):
    path.mkdir(parents=True, exist_ok=True, mode=0o700)
    if path.is_symlink() or not path.is_dir():
        raise InboxError('Use a regular directory for inbox files.')


def atomic_private(path, data, replace=False):
    private_directory(path.parent)
    descriptor, temporary = tempfile.mkstemp(prefix='.' + path.name + '-', dir=str(path.parent))
    try:
        with os.fdopen(descriptor, 'wb') as stream:
            stream.write(data)
            stream.flush()
            os.fsync(stream.fileno())
        if replace:
            if path.is_symlink():
                raise InboxError('Refusing to replace a symbolic link.')
            os.replace(temporary, path)
        else:
            # Atomic no-replace install; an interrupted write never looks complete.
            try:
                os.link(temporary, path)
            except FileExistsError:
                if path.is_symlink() or not path.is_file():
                    raise InboxError('Refusing to use a non-regular report file.') from None
    finally:
        if os.path.exists(temporary):
            os.unlink(temporary)


class Client:
    def __init__(self, endpoint, token):
        self.endpoint = endpoint_url(endpoint)
        if not isinstance(token, str) or not re.fullmatch(r'[!-~]{32,4096}', token):
            raise InboxError('Set a valid read key with configure or CLIPCLUB_DIAGNOSTICS_READ_TOKEN.')
        self.token = token
        self.opener = urllib.request.build_opener(NoRedirect())

    def get(self, path):
        request = urllib.request.Request(self.endpoint + path, headers={
            'Authorization': 'Bearer ' + self.token, 'Accept': 'application/json',
            'User-Agent': USER_AGENT,
        })
        try:
            with self.opener.open(request, timeout=20) as response:
                body = response.read(MAX_RESPONSE + 1)
                if len(body) > MAX_RESPONSE:
                    raise InboxError('The inbox returned an oversized response.')
                result = json.loads(body)
        except urllib.error.HTTPError as error:
            if error.code == 401:
                raise InboxError('The inbox rejected the read key.') from None
            raise InboxError('The inbox request failed (HTTP {}).'.format(error.code)) from None
        except (urllib.error.URLError, http.client.HTTPException, TimeoutError, OSError):
            raise InboxError('Could not reach the inbox. Check the connection and try again.') from None
        except (ValueError, UnicodeError):
            raise InboxError('The inbox returned invalid JSON.') from None
        return result

    def reports(self, session=None):
        cursor = None
        seen = set()
        while True:
            query = {}
            if session is not None:
                query['session'] = session
            if cursor:
                query['cursor'] = cursor
            page = self.get('/v1/reports?' + urllib.parse.urlencode(query))
            if not isinstance(page, dict) or not isinstance(page.get('reports'), list):
                raise InboxError('The inbox returned an invalid report list.')
            for report in page['reports']:
                if not isinstance(report, dict) or not isinstance(report.get('id'), str) or not REPORT_ID.fullmatch(report['id']):
                    raise InboxError('The inbox returned an invalid report ID.')
                yield report
            cursor = page.get('cursor')
            if cursor is None:
                break
            if not isinstance(cursor, str) or not cursor or len(cursor) > 4096 or cursor in seen:
                raise InboxError('The inbox returned an invalid pagination cursor.')
            seen.add(cursor)

    def report(self, report_id):
        if not REPORT_ID.fullmatch(report_id):
            raise InboxError('Invalid report ID.')
        result = self.get('/v1/reports/' + report_id)
        if (not isinstance(result, dict) or result.get('id') != report_id or
                not isinstance(result.get('report'), str)):
            raise InboxError('The inbox returned an invalid report.')
        return result


def pull(client, output, session=None):
    private_directory(output)
    downloaded = 0
    skipped = 0
    for item in client.reports(session):
        report_id = item['id']
        paths = [output / (report_id + extension) for extension in ('.json', '.txt')]
        for path in paths:
            if path.is_symlink() or (path.exists() and not path.is_file()):
                raise InboxError('Refusing to use a non-regular report file.')
        if all(path.exists() for path in paths):
            skipped += 1
            continue
        data = client.report(report_id)
        atomic_private(paths[0], (json.dumps(data, ensure_ascii=False, indent=2) + '\n').encode('utf-8'))
        atomic_private(paths[1], data['report'].encode('utf-8'))
        downloaded += 1
    return downloaded, skipped


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--config', type=Path, default=DEFAULT_CONFIG, help='Private config file')
    parser.add_argument('--endpoint', help='HTTPS inbox origin (overrides config)')
    commands = parser.add_subparsers(dest='command', required=True)
    commands.add_parser('configure', help='Save endpoint and a read key entered privately')
    listing = commands.add_parser('list', help='Print report metadata as JSON')
    listing.add_argument('--session', help='Exact test-session label')
    pulling = commands.add_parser('pull', help='Download new reports as JSON and text')
    pulling.add_argument('--session', help='Exact test-session label')
    pulling.add_argument('--output', type=Path, default=DEFAULT_OUTPUT)
    args = parser.parse_args(argv)
    config = read_config(args.config)
    endpoint = args.endpoint or os.environ.get('CLIPCLUB_DIAGNOSTICS_ENDPOINT') or config.get('endpoint')
    if args.command == 'configure':
        endpoint = endpoint_url(endpoint or input('Inbox URL: '))
        token = getpass.getpass('Read key: ').strip()
        Client(endpoint, token)
        atomic_private(args.config, (json.dumps({'endpoint': endpoint, 'read_token': token}) + '\n').encode('utf-8'), replace=True)
        print('Saved private inbox configuration.')
        return 0
    if not endpoint:
        raise InboxError('Run configure first or set CLIPCLUB_DIAGNOSTICS_ENDPOINT.')
    token = os.environ.get('CLIPCLUB_DIAGNOSTICS_READ_TOKEN') or config.get('read_token')
    client = Client(endpoint, token)
    if args.command == 'list':
        print(json.dumps(list(client.reports(args.session)), ensure_ascii=False, indent=2))
    else:
        downloaded, skipped = pull(client, args.output, args.session)
        print('Downloaded {} report(s); {} already present. {}'.format(downloaded, skipped, args.output))
    return 0


if __name__ == '__main__':
    try:
        sys.exit(main())
    except (InboxError, OSError) as error:
        # Transport errors are intentionally generalized before reaching this point.
        print('Inbox: ' + str(error), file=sys.stderr)
        sys.exit(1)
    except KeyboardInterrupt:
        sys.exit(130)
