#!/usr/bin/env python3
"""Verify the recovered packaged app with disposable profiles only."""
import json
import os
from pathlib import Path
import re
import signal
import socket
import subprocess
import sys
import tempfile
import time
import urllib.error
import urllib.request


def port():
    with socket.socket() as stream:
        stream.bind(('127.0.0.1', 0))
        return stream.getsockname()[1]


binary = Path(sys.argv[1]).resolve(strict=True)
environment = dict(os.environ)
environment.pop('CLIPCROP_FFPROBE', None)
with tempfile.TemporaryDirectory(prefix='clipcrop-points-package-') as folder:
    profile = Path(folder) / 'profile'
    saved_identity = None
    for attempt in range(2):
        http_port = port()
        log = open(Path(folder) / f'launch-{attempt}.log', 'w+')
        process = subprocess.Popen([
            str(binary), '--headless', '--data-dir', str(profile),
            '--http-port', str(http_port), '--p2p-port', str(port()),
            '--torrent-port', str(port()),
        ], env=environment, stdout=log, stderr=log)
        try:
            url = f'http://127.0.0.1:{http_port}'
            deadline = time.monotonic() + 30
            while True:
                if process.poll() is not None:
                    log.seek(0)
                    raise AssertionError(log.read())
                try:
                    html = urllib.request.urlopen(url, timeout=1).read().decode()
                    break
                except OSError:
                    if time.monotonic() > deadline:
                        raise
                    time.sleep(.1)
            token = re.search(r'name="api-token" content="([a-f0-9]+)"', html)[1]

            def request(route, data=None):
                headers = {'x-clipcrop-token': token}
                if data is not None:
                    headers['Content-Type'] = 'application/json'
                    data = json.dumps(data).encode()
                query = urllib.request.Request(url + '/api/' + route, data=data, headers=headers)
                with urllib.request.urlopen(query, timeout=5) as response:
                    return json.loads(response.read())

            state = request('state')
            assert state['balance'] == 100
            assert 'wallet' not in state and 'pool' not in state
            assert Path(state['data_folder']).resolve() == profile.resolve()
            assert json.loads((profile / 'state.json').read_text())['version'] == 1
            assert (profile / 'bin' / 'ffprobe').is_file(), 'Embedded validator was not installed'
            if attempt == 0:
                saved_identity = state['profile_id']
                settings = state['settings']
                settings['display_name'] = 'Recovered Points Check'
                request('settings', settings)
            else:
                assert state['profile_id'] == saved_identity
                assert state['settings']['display_name'] == 'Recovered Points Check'
            for route, data in [('wallet/mine', {}), ('roll', {'wager': '1'})]:
                try:
                    request(route, data)
                    raise AssertionError('Cryptocurrency endpoint survived: ' + route)
                except urllib.error.HTTPError as error:
                    assert error.code in (404, 405)
            assert request('state')['balance'] == 100
        finally:
            if process.poll() is None:
                process.send_signal(signal.SIGINT)
                try:
                    process.wait(8)
                except subprocess.TimeoutExpired:
                    process.kill()
                    process.wait()
            log.close()
print('PASS: packaged v0.1.0 points app, 100-point start, embedded validator, no crypto actions, saved identity/settings and restart; temporary profiles removed.')
