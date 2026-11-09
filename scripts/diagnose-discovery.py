#!/usr/bin/env python3
"""Inspect Linux peer discovery without changing settings or opening a profile."""
import argparse
import ipaddress
import json
import re
import select
import socket
import struct
import subprocess
import time

SERVICE = '_p2p._udp.local.'
GROUP = '224.0.0.251'
MAX_PACKET = 9000


def dns_name(packet, offset):
    labels, seen, end, expanded = [], set(), None, 0
    while True:
        if offset in seen or offset >= len(packet):
            raise ValueError('invalid DNS name pointer')
        seen.add(offset)
        length = packet[offset]
        if length & 0xc0 == 0xc0:
            if offset + 1 >= len(packet) or len(seen) > 128:
                raise ValueError('truncated DNS name pointer')
            end = end or offset + 2
            offset = ((length & 63) << 8) | packet[offset + 1]
            continue
        if length & 0xc0 or offset + 1 + length > len(packet):
            raise ValueError('invalid DNS label')
        offset += 1
        if not length:
            return '.'.join(labels).lower() + '.', end or offset
        expanded += length + 1
        if expanded > 254:
            raise ValueError('DNS name is too long')
        labels.append(packet[offset:offset + length].decode('ascii'))
        offset += length


def parse_response(packet):
    """Return advertised direct TCP addresses linked to the libp2p PTR answer."""
    if not 12 <= len(packet) <= MAX_PACKET:
        raise ValueError('invalid DNS packet size')
    _, flags, questions, answers, authorities, extra = struct.unpack_from('!6H', packet)
    if not flags & 0x8000:
        return []
    if flags & 0x7a0f or questions > 32 or answers + authorities + extra > 256:
        raise ValueError('unsupported DNS response')
    offset, targets, texts = 12, set(), []
    for _ in range(questions):
        _, offset = dns_name(packet, offset)
        offset += 4
        if offset > len(packet):
            raise ValueError('truncated DNS question')
    for _ in range(answers + authorities + extra):
        owner, offset = dns_name(packet, offset)
        if offset + 10 > len(packet):
            raise ValueError('truncated DNS record')
        kind, cls, ttl, size = struct.unpack_from('!HHIH', packet, offset)
        offset += 10
        end = offset + size
        if end > len(packet):
            raise ValueError('truncated DNS record data')
        if cls & 0x7fff == 1 and ttl:
            if kind == 12 and owner == SERVICE:
                target, consumed = dns_name(packet, offset)
                if consumed != end:
                    raise ValueError('invalid PTR record')
                targets.add(target)
            elif kind == 16:
                cursor = offset
                while cursor < end:
                    length = packet[cursor]
                    cursor += 1
                    if cursor + length > end:
                        raise ValueError('truncated TXT string')
                    texts.append((owner, packet[cursor:cursor + length]))
                    cursor += length
        offset = end
    endpoints = set()
    for owner, value in texts:
        if owner not in targets:
            continue
        value = value.removeprefix(b'"').removesuffix(b'"')
        match = re.fullmatch(rb'dnsaddr=(/ip([46])/([0-9A-Fa-f:.]+)/tcp/([0-9]{1,5})/p2p/([1-9A-HJ-NP-Za-km-z]{32,128}))', value)
        if match:
            try:
                ip = ipaddress.ip_address(match[3].decode('ascii'))
                if ip.version == int(match[2]) and 0 < int(match[4]) <= 65535:
                    endpoints.add(match[1].decode('ascii'))
            except ValueError:
                pass
    return sorted(endpoints)


def command(args):
    return subprocess.run(args, capture_output=True, text=True, timeout=2, check=True).stdout


def interfaces():
    try:
        rows = json.loads(command(['ip', '-j', '-4', 'address', 'show']))
        return sorted({(row['ifname'], str(ipaddress.IPv4Address(addr['local'])))
                       for row in rows if 'UP' in row.get('flags', []) and 'LOOPBACK' not in row.get('flags', [])
                       for addr in row.get('addr_info', []) if addr.get('family') == 'inet'}), None
    except (OSError, subprocess.SubprocessError, ValueError, KeyError):
        try:
            with socket.socket(socket.AF_INET, socket.SOCK_DGRAM) as route:
                route.connect(('1.1.1.1', 9))  # UDP connect chooses a route; it sends no packet.
                return [('default-route', route.getsockname()[0])], 'ip unavailable; only the default IPv4 route is checked.'
        except OSError as error:
            return [], f'Cannot enumerate IPv4 interfaces: {error}'


def probe(rows, seconds):
    query = struct.pack('!6H', 0, 0, 1, 0, 0, 0) + b''.join(bytes([len(label)]) + label.encode() for label in SERVICE.rstrip('.').split('.')) + b'\0\0\x0c\0\x01'
    sockets, found, errors, sources, sent, packets = [], set(), [], set(), 0, 0
    try:
        for name, ip in rows[:16]:
            stream = None
            try:
                stream = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
                stream.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
                if hasattr(socket, 'SO_REUSEPORT'):
                    stream.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEPORT, 1)
                stream.bind(('', 5353))
                stream.setsockopt(socket.IPPROTO_IP, socket.IP_ADD_MEMBERSHIP, socket.inet_aton(GROUP) + socket.inet_aton(ip))
                stream.setsockopt(socket.IPPROTO_IP, socket.IP_MULTICAST_IF, socket.inet_aton(ip))
                stream.setsockopt(socket.IPPROTO_IP, socket.IP_MULTICAST_TTL, 255)
                stream.setblocking(False)
                stream.sendto(query, (GROUP, 5353))
                sent += 1
                sockets.append(stream)
            except OSError as error:
                errors.append(f'{name} ({ip}): {error}')
                if stream is not None:
                    stream.close()
        deadline = time.monotonic() + seconds
        while sockets and packets < 512 and time.monotonic() < deadline:
            ready, _, _ = select.select(sockets, [], [], max(0, deadline - time.monotonic()))
            for stream in ready:
                try:
                    data, source = stream.recvfrom(MAX_PACKET + 1)
                    packets += 1
                    # libp2p sends mDNS responses from an interface-bound
                    # ephemeral socket. Validate their DNS contents, not the
                    # source port; announcements remain untrusted observations.
                    endpoints = parse_response(data)
                    if endpoints:
                        if len(sources) < 64:
                            sources.add(source[0])
                        found.update(endpoints[:max(0, 256 - len(found))])
                except (OSError, ValueError, UnicodeError):
                    continue
    finally:
        for stream in sockets:
            stream.close()
    return sent, packets, sorted(found), sorted(sources), errors


def interpretation(sent, endpoints, errors):
    if endpoints:
        return 'libp2p mDNS announcements received. If the app shows no peers, test TCP reachability to the other device.'
    if not sent:
        return 'No discovery query could be sent. Resolve the interface/socket errors above first.'
    return 'No libp2p mDNS endpoints observed. This alone cannot distinguish app, firewall, or Wi-Fi multicast isolation.' + (' Some interfaces also failed.' if errors else '')


def port(value):
    number = int(value)
    if not 1 <= number <= 65535:
        raise argparse.ArgumentTypeError('port must be 1–65535')
    return number


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--peer', type=ipaddress.IPv4Address, help='other device IPv4 address for explicit TCP checks')
    parser.add_argument('--p2p-port', type=port, default=42931)
    parser.add_argument('--torrent-port', type=port, default=42932)
    args = parser.parse_args()
    print('ClipClub local discovery diagnostic — keep ClipClub open on both devices.')
    rows, warning = interfaces()
    for name, ip in rows:
        print(f'IPv4 interface: {name} {ip}')
    if warning:
        print(warning)
    for kind, ports in [('TCP', (args.p2p_port, args.torrent_port)), ('UDP', (5353,))]:
        try:
            lines = command(['ss', '-ltn' if kind == 'TCP' else '-lun']).splitlines()
            matches = [line for line in lines[1:] if any(re.search(rf':{p}\s', line) for p in ports)]
            print(f'{kind} listeners: ' + ('; '.join(matches[:12]) or 'none on expected ports'))
        except (OSError, subprocess.SubprocessError):
            print(f'{kind} listener inspection unavailable (ss missing or failed).')
    sent, packets, endpoints, sources, errors = probe(rows, 5)
    print(f'mDNS queries sent: {sent}; packets observed: {packets}; unique endpoints: {len(endpoints)}; source IPs: {", ".join(sources) or "none"}')
    for text in errors + endpoints:
        print(text)
    print(interpretation(sent, endpoints, errors))
    print('Announcements are unverified libp2p observations and can include this device or other applications.')
    if args.peer:
        for label, number in [('peer mesh', args.p2p_port), ('BitTorrent', args.torrent_port)]:
            try:
                with socket.create_connection((str(args.peer), number), timeout=3):
                    print(f'TCP {label} {args.peer}:{number}: reachable (protocol not checked)')
            except OSError as error:
                print(f'TCP {label} {args.peer}:{number}: {error}')
    else:
        print('TCP checks skipped; use --peer OTHER_DEVICE_IPV4 to check the other device explicitly.')


if __name__ == '__main__':
    main()
