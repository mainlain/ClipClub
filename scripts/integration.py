#!/usr/bin/env python3
"""Local, real-process mesh/BitTorrent/HTTP integration test. No hosted services.
Usage: python3 scripts/integration.py [path/to/clipclub] [path/to/ffmpeg]
"""
import hashlib
import http.client
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

ROOT = Path(__file__).resolve().parent.parent
BINARY = Path(sys.argv[1] if len(sys.argv) > 1 else ROOT / 'target/debug/clipclub').resolve()
FFMPEG = Path(sys.argv[2] if len(sys.argv) > 2 else ROOT / '.build/ffmpeg').resolve()
WORK = Path(tempfile.mkdtemp(prefix='clipcrop-e2e-'))
PROCESSES = []
ENVIRONMENT = dict(os.environ)
for key in ('CLIPCLUB_MEGAETH_CHAIN_ID', 'CLIPCLUB_JOAH_CONTRACT', 'CLIPCLUB_MEGAETH_RPC_URL'):
    ENVIRONMENT.pop(key, None)
# Keep peer/media integration independent of the deployed token and public RPC.
ENVIRONMENT['CLIPCLUB_JOAH_CONTRACT'] = ''

def free_port():
    with socket.socket() as stream:
        stream.bind(('127.0.0.1', 0))
        return stream.getsockname()[1]

def wait_for(check, description, timeout=40):
    start = time.monotonic()
    last = None
    while time.monotonic() - start < timeout:
        try:
            last = check()
            if last:
                return last
        except (OSError, KeyError, AssertionError) as error:
            last = str(error)
        time.sleep(.2)
    raise AssertionError(f'{description}: timed out, last={last}')

class Client:
    def __init__(self, name):
        self.name = name
        self.root = WORK / name
        self.ports = [free_port() for _ in range(3)]
        self.start()

    def start(self):
        self.log = open(WORK / f'{self.name}.log', 'a')
        self.process = subprocess.Popen([str(BINARY), '--headless', '--data-dir', str(self.root), '--http-port', str(self.ports[0]), '--p2p-port', str(self.ports[1]), '--torrent-port', str(self.ports[2])], env=ENVIRONMENT, stdout=self.log, stderr=self.log)
        PROCESSES.append(self)
        self.url = f'http://127.0.0.1:{self.ports[0]}'
        def load_token():
            html = urllib.request.urlopen(self.url, timeout=2).read().decode()
            self.token = re.search(r'name="api-token" content="([a-f0-9]+)"', html)[1]
            return True
        wait_for(load_token, f'{self.name} startup')
        self.identity = self.state()['profile_id']

    def stop(self):
        if self.process.poll() is None:
            self.process.send_signal(signal.SIGINT)
            try:
                self.process.wait(8)
            except subprocess.TimeoutExpired:
                self.process.kill()
                self.process.wait()
        self.log.close()

    def request(self, route, data=None, raw=None, content_type=None):
        headers = {'x-clipcrop-token': self.token}
        if data is not None:
            raw = json.dumps(data).encode()
            headers['Content-Type'] = 'application/json'
        if content_type:
            headers['Content-Type'] = content_type
        req = urllib.request.Request(self.url + '/api/' + route, data=raw, headers=headers)
        with urllib.request.urlopen(req, timeout=55) as response:
            return json.loads(response.read())

    def state(self):
        return self.request('state')

    def connect(self, other):
        settings = self.state()['settings']
        settings['bootstrap_peers'] = [f'/ip4/127.0.0.1/tcp/{other.ports[1]}/p2p/{other.identity}']
        self.request('settings', settings)

    def upload(self, path, name=None):
        boundary = 'clipcrop-integration-boundary'
        raw = (f'--{boundary}\r\nContent-Disposition: form-data; name="video"; filename="{name or path.name}"\r\nContent-Type: application/octet-stream\r\n\r\n'.encode() + path.read_bytes() + f'\r\n--{boundary}--\r\n'.encode())
        return self.request('upload', raw=raw, content_type=f'multipart/form-data; boundary={boundary}')

def video(name, source, seconds, webm=False):
    output = WORK / name
    codec = 'libvpx-vp9' if webm else 'libx264'
    subprocess.run([str(FFMPEG), '-v', 'error', '-f', 'lavfi', '-i', source, '-t', str(seconds), '-c:v', codec, '-pix_fmt', 'yuv420p', '-metadata', f'title={name}', '-y', str(output)], check=True)
    return output

def assert_http_error(action, contains=None):
    try:
        action()
    except urllib.error.HTTPError as error:
        body = error.read().decode()
        assert error.code in (400, 401, 403, 413, 422), (error.code, body)
        if contains:
            assert contains.lower() in body.lower(), body
        return
    raise AssertionError('Expected rejection')

try:
    videos = [video(f'clip-{n:02}.mp4', f'color=c=0x{n+25:02x}8065:s=96x64:r=10', .3) for n in range(12)]
    webm = video('terracotta.webm', 'color=c=0xdb653d:s=96x64:r=10', .3, True)
    wide = video('too-wide.mp4', 'color=s=2162x16:r=1', 1)
    long = video('too-long.mp4', 'color=s=16x16:r=1', 181)
    fake = WORK / 'fake.mp4'; fake.write_bytes(b'not a video')
    large = WORK / 'large.mp4'; large.write_bytes(b'0' * 8_000_001)
    alice, bob = Client('Alice'), Client('Bob')
    for client in (alice, bob):
        crypto = client.state()['crypto']
        assert crypto['chain_id'] == 4326 and not crypto['configured']
        assert crypto['name'] == '좋아' and crypto['symbol'] == '✨' and crypto['decimals'] == 6
        assert crypto['balance_units'] == '0' and crypto['total_supply_units'] == '0'
        assert crypto['wallet'] is None and crypto['contract'] is None
        assert 'balance' not in client.state(), 'Legacy points were exposed as spendable money'
    bob.connect(alice)
    wait_for(lambda: len(alice.state()['peers']) == 1 and len(bob.state()['peers']) == 1, 'mutual presence')
    assert_http_error(lambda: urllib.request.urlopen(alice.url + '/api/state'), 'Invalid local session')
    bad = urllib.request.Request(alice.url + '/api/state', headers={'x-clipcrop-token':alice.token, 'Origin':'https://untrusted.example'})
    assert_http_error(lambda: urllib.request.urlopen(bad))
    for file, text in [(wide, 'resolution'), (long, 'duration'), (fake, 'signature'), (large, None)]:
        assert_http_error(lambda file=file: alice.upload(file), text)
    assert not alice.state()['library'], 'Rejected uploads entered collection'
    by_hash = {}
    for file in videos + [webm]:
        clip = alice.upload(file)['clip']
        by_hash[clip['id']] = file
    first = alice.upload(videos[0])
    assert first['duplicate'] and len(alice.state()['library']) == 13
    library = alice.state()['library']
    assert all(row['added_at_ms'] > 0 and 'points_earned' not in row and row['average_rating'] is None and row['rating_count'] == 0 for row in library)
    assert len({row['added_at_ms'] for row in library}) == 13
    wait_for(lambda: len(bob.state()['queue']) == 10, 'ten preloaded clips', 60)
    bob.request('session', {'active':True})
    current = bob.state()['queue'][0]
    assert current['id'] == hashlib.sha256(by_hash[current['id']].read_bytes()).hexdigest()
    # The actual player endpoint must support bounded byte-range seeking.
    req = urllib.request.Request(bob.url + '/api/video/' + current['id'], headers={'x-clipcrop-token':bob.token,'Range':'bytes=0-15'})
    with urllib.request.urlopen(req) as response:
        assert response.status == 206 and len(response.read()) == 16
    # Unconfigured applications cannot create paid ratings or mutate a balance.
    before = bob.state()
    assert_http_error(lambda: bob.request('rate', {'id':current['id'], 'score':5}))
    assert bob.state()['queue'][0]['id'] == current['id']
    assert bob.state()['crypto']['balance_units'] == '0'
    # Free skips still produce signed, persistent peer history.
    result = bob.request('rate', {'id':current['id'], 'score':0})
    assert result['score'] == 0 and result['reward_per_seeder'] == 0
    assert bob.state()['temporary_seeds'] == 0
    wait_for(lambda: len(bob.state()['queue']) == 10, 'queue refills after skip')
    assert_http_error(lambda: bob.request('rate', {'id':current['id'], 'score':0}))
    # Identical bytes from another user join the same deterministic swarm.
    promoted = bob.upload(by_hash[current['id']])
    assert promoted['duplicate'] and promoted['clip']['info_hash'] == current['info_hash']
    assert bob.state()['library'][0]['seeding']
    promoted_row = bob.state()['library'][0]
    assert promoted_row['average_rating'] is None and promoted_row['rating_count'] == 0
    assert 'points_earned' not in promoted_row
    bob.upload(by_hash[current['id']])
    assert bob.state()['library'][0]['added_at_ms'] == promoted_row['added_at_ms']
    bob.request('session', {'active':False})
    charlie = Client('Charlie'); charlie.connect(alice)
    settings = charlie.state()['settings']
    settings['bootstrap_peers'].append(f'/ip4/127.0.0.1/tcp/{bob.ports[1]}/p2p/{bob.identity}')
    charlie.request('settings', settings)
    wait_for(lambda: len(charlie.state()['peers']) == 2, 'three peer swarm')
    wait_for(lambda: len(charlie.state()['queue']) >= 1, 'third peer has ready clip')
    charlie.request('session', {'active':True})
    # Pass other clips free until the shared duplicate clip is reached.
    deadline = time.monotonic() + 40
    while time.monotonic() < deadline:
        ready = charlie.state()['queue']
        if not ready:
            time.sleep(.2); continue
        chosen = ready[0]
        if chosen['id'] == current['id']:
            break
        charlie.request('rate', {'id':chosen['id'], 'score':0})
    else:
        raise AssertionError('Shared duplicate clip never entered queue')
    assert_http_error(lambda: charlie.request('rate', {'id':current['id'], 'score':5}))
    result = charlie.request('rate', {'id':current['id'], 'score':0})
    assert result['reward_per_seeder'] == 0
    assert charlie.state()['crypto']['balance_units'] == '0'
    # Skips never create a rating or earnings, regardless of the seeder count.
    for client in (alice, bob):
        assert all(row['average_rating'] is None and row['rating_count'] == 0 and 'points_earned' not in row for row in client.state()['library'])
    charlie.request('session', {'active':False})
    assert charlie.state()['temporary_seeds'] == 0
    # Signed free-skip histories propagate through the real mesh.
    def peer_has_skips():
        peer_receipts = json.loads((alice.root / 'state.json').read_text())['receipts']
        senders = {receipt['payload']['sender'] for receipt in peer_receipts}
        return bob.identity in senders and charlie.identity in senders
    wait_for(peer_has_skips, 'peer accepts signed skip histories', 60)
    # Restart preserves identity, watched history and permanent seeding after relocation.
    bob_identity = bob.identity
    bob.stop()
    moved_root = WORK / "Bob-moved"
    bob.root.rename(moved_root)
    bob.root = moved_root
    bob.start()
    assert bob.identity == bob_identity
    assert bob.state()['crypto']['balance_units'] == '0'
    assert not bob.state()['crypto']['configured']
    assert len(bob.state()['library']) == 1 and bob.state()['library'][0]['seeding']
    restored = bob.state()['library'][0]
    assert restored['added_at_ms'] == promoted_row['added_at_ms']
    assert restored['average_rating'] is None and restored['rating_count'] == 0
    wait_for(lambda: bool(bob.state()['queue']), 'queue starts again after restart')
    assert all(item['id'] != current['id'] for item in bob.state()['queue'])
    watched = json.loads((bob.root / 'state.json').read_text())['watched']
    assert current['id'] in watched

    # The old floating-point wager endpoint must never generate local randomness.
    before_roll = bob.state()
    for payload in ({'wager':0}, {'wager':-1}, {'wager':1}, {'wager':.000001}, {}, {'wager':'1'}, {'wager':1,'multiplier':9}, {'transaction_hash':'invalid'}):
        assert_http_error(lambda payload=payload: bob.request('roll', payload))
        unchanged = bob.state()
        assert unchanged['crypto']['balance_units'] == '0'
        assert unchanged['roll'] == before_roll['roll'] is None
    assert not any(receipt['payload'].get('roll') for receipt in json.loads((bob.root / 'state.json').read_text())['receipts'])
    # A new free skip still appends to the signed history after restart.
    bob.request('session', {'active':True})
    next_clip = wait_for(lambda: bob.state()['queue'], 'clip ready after restart')[0]
    bob.request('rate', {'id':next_clip['id'], 'score':0})
    bob.request('session', {'active':False})
    bob.stop()
    bob.start()
    assert bob.identity == bob_identity
    assert bob.state()['crypto']['balance_units'] == '0' and bob.state()['roll'] is None
    assert next_clip['id'] in json.loads((bob.root / 'state.json').read_text())['watched']

    # Duplicate-profile lock prevents simultaneous spending from one profile.
    duplicate = subprocess.run([str(BINARY),'--headless','--data-dir',str(bob.root)], env=ENVIRONMENT, capture_output=True, text=True, timeout=5)
    assert duplicate.returncode != 0 and 'already open' in duplicate.stderr
    print('PASS: real three-peer mesh,13 uploads,MP4/WebM,hostile-file rejection,10-clip refill,range playback,free signed skips,dedup,seeding,session cleanup,restart,portable profile relocation,no initial token grants,no paid ratings without chain payment,no local RNG rolls,skip gossip,and profile lock.')
    print('Integration logs:', WORK)
finally:
    for client in reversed(PROCESSES):
        try:
            client.stop()
        except Exception:
            pass
