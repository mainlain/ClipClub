#!/usr/bin/env python3
"""Verify the packaged app with disposable profiles only.
Usage: python3 scripts/package-smoke.py path/to/clipclub
"""
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
for key in ('CLIPCLUB_MEGAETH_CHAIN_ID', 'CLIPCLUB_JOAH_CONTRACT', 'CLIPCLUB_MEGAETH_RPC_URL'):
    environment.pop(key, None)
# Exercise the explicit offline mode without depending on the live token RPC.
environment['CLIPCLUB_JOAH_CONTRACT'] = ''
with tempfile.TemporaryDirectory(prefix='clipclub-package-') as folder:
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
            crypto = state['crypto']
            assert crypto['chain_id'] == 4326 and not crypto['configured']
            assert crypto['name'] == '좋아' and crypto['symbol'] == '✨' and crypto['decimals'] == 6
            assert crypto['balance_units'] == crypto['total_supply_units'] == '0'
            assert crypto['wallet'] is None and crypto['contract'] is None
            assert 'balance' not in state, 'Legacy points were exposed as token balance'
            assert state['roll'] is None
            assert 'data-page="home"' in html
            assert 'id="library-sort"' in html and 'id="roll-wager"' in html
            assert 'id="install-update"' in html and 'id="check-updates"' in html
            assert state['update']['current_version'] == state['version']
            assert state['update']['phase'] == 'idle'
            try:
                request('update/open', {'target': 'file:///untrusted'})
                raise AssertionError('Client-selected update URL was accepted')
            except urllib.error.HTTPError as error:
                assert error.code == 400
            assert Path(state['data_folder']).resolve() == profile.resolve()
            assert json.loads((profile / 'state.json').read_text())['version'] == 1
            assert (profile / 'bin' / 'ffprobe').is_file(), 'Embedded validator was not installed'
            if attempt == 0:
                saved_identity = state['profile_id']
            else:
                assert state['profile_id'] == saved_identity
            assert 'settings' not in state
            assert 'id="bootstrap-peers"' not in html
            try:
                request('settings', {'bootstrap_peers': []})
                raise AssertionError('Manual peer settings endpoint is still available')
            except urllib.error.HTTPError as error:
                assert error.code == 404
            for payload in ({'wager':1}, {'wager':1,'multiplier':9}, {'transaction_hash':'invalid'}):
                try:
                    request('roll', payload)
                    raise AssertionError('Local or client-selected roll was accepted')
                except urllib.error.HTTPError as error:
                    assert error.code in (400, 422)
            try:
                request('rate', {'id':'0' * 64, 'score':5})
                raise AssertionError('Paid rating without chain payment was accepted')
            except urllib.error.HTTPError as error:
                assert error.code in (400, 422)
            assert request('state')['crypto']['balance_units'] == '0'
            assert not json.loads((profile / 'state.json').read_text())['receipts']
        finally:
            if process.poll() is None:
                process.send_signal(signal.SIGINT)
                try:
                    process.wait(8)
                except subprocess.TimeoutExpired:
                    process.kill()
                    process.wait()
            log.close()
print('PASS: compact menu, library sorting, 좋아 metadata, default MegaETH mainnet, zero token grants, no local RNG rolls or unpaid positive ratings, embedded validator, identity persistence, no manual peer controls; temporary profiles removed.')
