#!/usr/bin/env python3
"""Local, real-process mesh/BitTorrent/HTTP integration test. No hosted services.
Usage: python3 scripts/integration.py [path/to/clipcrop] [path/to/ffmpeg]
"""
import hashlib
import http.client
import json
import math
from decimal import Decimal
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
BINARY = Path(sys.argv[1] if len(sys.argv) > 1 else ROOT / 'target/debug/clipcrop').resolve()
FFMPEG = Path(sys.argv[2] if len(sys.argv) > 2 else ROOT / '.build/ffmpeg').resolve()
WORK = Path(tempfile.mkdtemp(prefix='clipcrop-e2e-'))
PROCESSES = []

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
        self.process = subprocess.Popen([str(BINARY), '--headless', '--data-dir', str(self.root), '--http-port', str(self.ports[0]), '--p2p-port', str(self.ports[1]), '--torrent-port', str(self.ports[2])], stdout=self.log, stderr=self.log)
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
        settings['display_name'] = self.name
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
    assert alice.state()['balance'] == bob.state()['balance'] == 100
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
    assert all(row['added_at_ms'] > 0 and row['points_earned'] == 0 and row['average_rating'] is None and row['rating_count'] == 0 for row in library)
    assert len({row['added_at_ms'] for row in library}) == 13
    wait_for(lambda: len(bob.state()['queue']) == 10, 'ten preloaded clips', 60)
    bob.request('session', {'active':True})
    current = bob.state()['queue'][0]
    assert current['id'] == hashlib.sha256(by_hash[current['id']].read_bytes()).hexdigest()
    # The actual player endpoint must support bounded byte-range seeking.
    req = urllib.request.Request(bob.url + '/api/video/' + current['id'], headers={'x-clipcrop-token':bob.token,'Range':'bytes=0-15'})
    with urllib.request.urlopen(req) as response:
        assert response.status == 206 and len(response.read()) == 16
    result = bob.request('rate', {'id':current['id'],'score':5})
    assert result['seeders'] == 1 and result['reward_per_seeder'] == 5
    wait_for(lambda: abs(alice.state()['balance'] - 105) < .000001, 'lone seeder credited')
    rated = next(row for row in alice.state()['library'] if row['clip']['id'] == current['id'])
    assert rated['points_earned'] == rated['average_rating'] == 5 and rated['rating_count'] == 1
    assert bob.state()['balance'] == 95 and bob.state()['temporary_seeds'] == 1
    wait_for(lambda: len(bob.state()['queue']) == 10, 'queue refills after rating')
    assert_http_error(lambda: bob.request('rate', {'id':current['id'],'score':5}))
    # Identical bytes from another user join the same deterministic swarm.
    promoted = bob.upload(by_hash[current['id']])
    assert promoted['duplicate'] and promoted['clip']['info_hash'] == current['info_hash']
    assert bob.state()['temporary_seeds'] == 0 and bob.state()['library'][0]['seeding']
    promoted_row = bob.state()['library'][0]
    assert promoted_row['average_rating'] == 5 and promoted_row['rating_count'] == 1 and promoted_row['points_earned'] == 0
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
    # Pass other clips free until the duplicate clip is reached.
    deadline = time.monotonic() + 40
    while time.monotonic() < deadline:
        ready = charlie.state()['queue']
        if not ready:
            time.sleep(.2); continue
        chosen = ready[0]
        if chosen['id'] == current['id']:
            break
        charlie.request('rate', {'id':chosen['id'],'score':0})
    else:
        raise AssertionError('Shared duplicate clip never entered queue')
    result = charlie.request('rate', {'id':current['id'],'score':5})
    assert result['seeders'] == 2, result
    assert abs(result['reward_per_seeder'] - 3.726779) < .000001, result
    wait_for(lambda: abs(alice.state()['balance'] - 108.726779) < .000001, 'multiple seeder payout Alice')
    wait_for(lambda: abs(bob.state()['balance'] - 98.726779) < .000001, 'multiple seeder payout Bob')
    assert charlie.state()['balance'] == 95
    for client, points in [(alice, 8.726779), (bob, 3.726779)]:
        rated = next(row for row in client.state()['library'] if row['clip']['id'] == current['id'])
        assert rated['average_rating'] == 5 and rated['rating_count'] == 2
        assert abs(rated['points_earned'] - points) < .000001
    # Other clips were only skipped and must remain unrated.
    assert all(row['average_rating'] is None and row['rating_count'] == 0 and row['points_earned'] == 0 for row in alice.state()['library'] if row['clip']['id'] != current['id'])
    charlie.request('session', {'active':False})
    assert charlie.state()['temporary_seeds'] == 0
    # Restart preserves wallet, identity, watched history and permanent seeding.
    bob_identity = bob.identity
    bob.stop()
    moved_root = WORK / "Bob-moved"
    bob.root.rename(moved_root)
    bob.root = moved_root
    bob.start()
    assert bob.identity == bob_identity
    assert abs(bob.state()['balance'] - 98.726779) < .000001
    assert len(bob.state()['library']) == 1 and bob.state()['library'][0]['seeding']
    restored = bob.state()['library'][0]
    assert restored['added_at_ms'] == promoted_row['added_at_ms']
    assert restored['average_rating'] == 5 and restored['rating_count'] == 2
    assert abs(restored['points_earned'] - 3.726779) < .000001
    wait_for(lambda: bool(bob.state()['queue']), 'queue starts again after restart')
    assert all(item['id'] != current['id'] for item in bob.state()['queue'])

    # Rejected wagers leave both the wallet and the last roll untouched.
    before_roll = bob.state()
    for wager in [0, -1, before_roll['balance'] + 1, .0000001]:
        assert_http_error(lambda wager=wager: bob.request('roll', {'wager':wager}))
        unchanged = bob.state()
        assert unchanged['balance'] == before_roll['balance']
        assert unchanged['roll'] == before_roll['roll']
    assert_http_error(lambda: bob.request('roll', {}))
    assert_http_error(lambda: bob.request('roll', {'wager':'1'}))

    # Exercise real random draws. Core tests exhaust all 1023 tickets; here small
    # wagers let the HTTP path reach loss, missing-tier, and watched/owned playback.
    # A tier-5 draw has probability 16/1023; 500 attempts keep failure below 0.04%.
    wallet = Decimal(str(before_roll['balance']))
    wager = Decimal('.001')
    seen_zero = seen_missing = seen_owned = False
    roll_ids = set()
    for _ in range(500):
        result = bob.request('roll', {'wager':float(wager)})
        multiplier = result['multiplier']
        assert isinstance(multiplier, int) and 0 <= multiplier <= 9, result
        assert Decimal(str(result['wager'])) == wager, result
        assert Decimal(str(result['payout'])) == wager * multiplier, result
        wallet += wager * (multiplier - 1)
        assert Decimal(str(result['balance'])) == wallet, result
        assert result['id'] not in roll_ids, 'A new roll reused an existing receipt'
        roll_ids.add(result['id'])
        assert bob.state()['roll']['id'] == result['id']
        if multiplier == 0:
            assert result['clip'] is None, result
            seen_zero = True
        elif multiplier == 5:
            # Bob rated this clip before promoting it: both watched and owned
            # content must be eligible, even though Rate excludes it.
            playable = wait_for(lambda: bob.state()['roll']['clip'], 'roll clip becomes playable')
            assert playable['id'] == current['id'], playable
            req = urllib.request.Request(bob.url + '/api/video/' + playable['id'], headers={'x-clipcrop-token':bob.token, 'Range':'bytes=0-15'})
            with urllib.request.urlopen(req) as response:
                assert response.status == 206 and len(response.read()) == 16
            seen_owned = True
        else:
            assert result['clip'] is None and result['message'], result
            seen_missing = True
        if seen_zero and seen_missing and seen_owned:
            break
    assert seen_zero and seen_missing and seen_owned, 'Random roll sampling did not reach all integration cases; rerun the smoke test'
    after_roll = bob.state()
    assert Decimal(str(after_roll['balance'])) == wallet
    assert after_roll['temporary_seeds'] == 0, 'Roll unexpectedly started temporary rating seeds'
    rolled_library = next(row for row in after_roll['library'] if row['clip']['id'] == current['id'])
    assert rolled_library['rating_count'] == 2 and rolled_library['average_rating'] == 5
    assert all(item['id'] != current['id'] for item in after_roll['queue'])

    # Gossip validates the roll chain, then accepts a rating following that chain.
    def peer_has_rolls():
        saved = json.loads((alice.root / 'state.json').read_text())
        return roll_ids.issubset({receipt['id'] for receipt in saved['receipts']})
    wait_for(peer_has_rolls, 'peer accepts mixed rating and roll history', 60)
    bob.request('session', {'active':True})
    next_clip = wait_for(lambda: bob.state()['queue'], 'clip ready for rating after rolls')[0]
    alice_balance = alice.state()['balance']
    bob.request('rate', {'id':next_clip['id'], 'score':1})
    wallet -= 1
    wait_for(lambda: abs(alice.state()['balance'] - alice_balance - 1) < .000001, 'peer accepts rating after roll receipts')
    bob.request('session', {'active':False})
    assert Decimal(str(bob.state()['balance'])) == wallet
    bob.stop()
    bob.start()
    assert Decimal(str(bob.state()['balance'])) == wallet, 'Roll wallet did not survive restart'
    assert bob.identity == bob_identity

    # Charlie watched the tier-5 clip, ended Rate, and never uploaded it. Roll
    # must fetch it again from a peer using its separate validated download cache.
    charlie_before = charlie.state()
    charlie_watched = json.loads((charlie.root / 'state.json').read_text())['watched']
    assert current['id'] in charlie_watched
    assert not any(row['clip']['id'] == current['id'] for row in charlie_before['library'])
    clip_filename = f"{current['id']}.{current['extension']}"
    assert not (charlie.root / 'cache' / clip_filename).exists()
    assert not (charlie.root / 'roll-cache' / clip_filename).exists()
    for _ in range(500):
        remote_roll = charlie.request('roll', {'wager':.001})
        if remote_roll['multiplier'] == 5:
            break
    else:
        raise AssertionError('Random roll sampling did not reach watched remote tier-5 playback; rerun the smoke test')
    remote_clip = wait_for(lambda: charlie.state()['roll']['clip'], 'watched roll clip downloads from a peer', 60)
    assert remote_clip['id'] == current['id'], remote_clip
    roll_cache = charlie.root / 'roll-cache' / clip_filename
    assert roll_cache.is_file(), 'Roll did not prepare its separate cache file'
    assert hashlib.sha256(roll_cache.read_bytes()).hexdigest() == current['id']
    req = urllib.request.Request(charlie.url + '/api/video/' + current['id'], headers={'x-clipcrop-token':charlie.token})
    with urllib.request.urlopen(req) as response:
        assert response.status == 200
        assert hashlib.sha256(response.read()).hexdigest() == current['id']
    charlie_after = charlie.state()
    assert charlie_after['roll']['id'] == remote_roll['id']
    assert charlie_after['library'] == charlie_before['library']
    assert charlie_after['temporary_seeds'] == charlie_before['temporary_seeds'] == 0
    assert not charlie_after['session']
    assert json.loads((charlie.root / 'state.json').read_text())['watched'] == charlie_watched

    # Duplicate-profile lock prevents simultaneous spending from one profile.
    duplicate = subprocess.run([str(BINARY),'--headless','--data-dir',str(bob.root)], capture_output=True, text=True, timeout=5)
    assert duplicate.returncode != 0 and 'already open' in duplicate.stderr
    print('PASS: real three-peer mesh,13 uploads,MP4/WebM,hostile-file rejection,10-clip refill,range playback,zero passes,100-point starts,lone/multiple payouts,dedup,promotion,session cleanup,restart,portable profile relocation,roll wager validation and payouts,watched/owned roll playback,validated watched remote roll downloads,mixed roll/rating gossip,roll wallet persistence,and profile lock.')
    print('Integration logs:', WORK)
finally:
    for client in reversed(PROCESSES):
        try:
            client.stop()
        except Exception:
            pass
