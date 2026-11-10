#!/usr/bin/env python3
"""Verify a real desktop process uploads to a deployed diagnostic inbox.

Uses private .build/diagnostic-inbox-secrets.json by default. Sends one synthetic
report, retained under the bucket's normal expiry policy. Never prints tokens.
"""
import argparse
import json
import os
from pathlib import Path
import re
import signal
import socket
import subprocess
import tempfile
import time
import urllib.error
import urllib.request


ROOT = Path(__file__).resolve().parent.parent


def free_port():
    with socket.socket() as stream:
        stream.bind(('127.0.0.1', 0))
        return stream.getsockname()[1]


def request(url, token=None, body=None, method=None, local=False):
    headers = {'User-Agent': 'ClipClub-Diagnostic-Integration/0.1'}
    if token:
        headers['x-clipcrop-token' if local else 'Authorization'] = token if local else 'Bearer ' + token
    data = None if body is None else json.dumps(body).encode()
    if data is not None:
        headers['Content-Type'] = 'application/json'
    req = urllib.request.Request(url, data=data, headers=headers, method=method)
    with urllib.request.urlopen(req, timeout=20) as response:
        return json.loads(response.read())


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, default=ROOT / 'target/debug/clipclub')
    parser.add_argument('--credentials', type=Path, default=ROOT / '.build/diagnostic-inbox-secrets.json')
    args = parser.parse_args()
    credentials = json.loads(args.credentials.read_text())
    endpoint = credentials['endpoint'].rstrip('/')
    with tempfile.TemporaryDirectory(prefix='clipclub-inbox-e2e-') as work:
        ports = [free_port() for _ in range(3)]
        local = f'http://127.0.0.1:{ports[0]}'
        environment = dict(os.environ, CLIPCLUB_JOAH_CONTRACT='')
        with open(Path(work) / 'process.log', 'w') as log:
            process = subprocess.Popen([
                str(args.binary.resolve()), '--headless', '--data-dir', work,
                '--http-port', str(ports[0]), '--p2p-port', str(ports[1]),
                '--torrent-port', str(ports[2]),
            ], env=environment, stdout=log, stderr=log)
            try:
                token = None
                deadline = time.monotonic() + 40
                while time.monotonic() < deadline:
                    if process.poll() is not None:
                        raise RuntimeError('Desktop process exited before startup; inspect its test log.')
                    try:
                        with urllib.request.urlopen(local, timeout=1) as response:
                            token = re.search(r'name="api-token" content="([a-f0-9]+)"', response.read().decode())[1]
                        break
                    except (OSError, TypeError):
                        time.sleep(.2)
                assert token, 'Desktop process did not start.'
                route = local + '/api/diagnostics/inbox'
                try:
                    request(route)
                    raise AssertionError('Local inbox settings were readable without a session token.')
                except urllib.error.HTTPError as error:
                    assert error.code == 401, error.code
                initial = request(route, token, local=True)
                assert initial['enabled'] is False and initial['pending'] == 0
                session = 'inbox-e2e-' + str(int(time.time()))
                status = request(route, token, {
                    'enabled': True, 'endpoint': endpoint,
                    'upload_token': credentials['upload_token'],
                    'device': 'Synthetic integration test', 'session': session,
                }, local=True)
                assert 'upload_token' not in status
                text = 'ClipClub diagnostic inbox integration verification\nSynthetic report; no wallet or profile contents.\n'
                result = request(route + '/reports', token, {
                    'suite': 'checklist', 'generated_at_ms': int(time.time() * 1000), 'report': text,
                }, local=True)
                report_id = result.get('id') or result.get('queued_id') or result.get('last_report_id')
                deadline = time.monotonic() + 45
                while time.monotonic() < deadline:
                    status = request(route, token, local=True)
                    if status['pending'] == 0 and status.get('last_report_id'):
                        report_id = report_id or status['last_report_id']
                        break
                    request(route + '/retry', token, {}, local=True)
                    time.sleep(.5)
                assert status['pending'] == 0 and report_id, 'Report did not leave the durable queue.'
                report_url = endpoint + '/v1/reports/' + report_id
                report = request(report_url, credentials['read_token'])
                assert report['report'] == text and report['session'] == session
                assert report['device'] == 'Synthetic integration test'
                assert request(report_url, credentials['upload_token'], report, 'PUT') == {'id': report_id}
                changed = dict(report, report='Changed report must not replace the original.')
                try:
                    request(report_url, credentials['upload_token'], changed, 'PUT')
                    raise AssertionError('A retry overwrote an existing report.')
                except urllib.error.HTTPError as error:
                    assert error.code == 409, error.code
                assert request(report_url, credentials['read_token'])['report'] == text
                for unauthorized in (None, credentials['upload_token']):
                    try:
                        request(report_url, unauthorized)
                        raise AssertionError('Report was readable without the reader credential.')
                    except urllib.error.HTTPError as error:
                        assert error.code in (401, 403), error.code
                print('PASS: real desktop process → durable queue → HTTPS Worker → private R2 → authenticated read; upload-only access cannot read reports.')
                print('Synthetic test report: ' + report_id)
            finally:
                if process.poll() is None:
                    process.send_signal(signal.SIGINT)
                    try:
                        process.wait(timeout=8)
                    except subprocess.TimeoutExpired:
                        process.kill()
                        process.wait()


if __name__ == '__main__':
    main()
