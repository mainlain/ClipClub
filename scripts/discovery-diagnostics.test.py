#!/usr/bin/env python3
"""Exercise bounded DNS parsing and discovery diagnosis, without network access."""
import importlib.util
from pathlib import Path
import struct
import unittest
from unittest.mock import Mock, patch

spec = importlib.util.spec_from_file_location('diagnostic', Path(__file__).with_name('diagnose-discovery.py'))
diagnostic = importlib.util.module_from_spec(spec)
spec.loader.exec_module(diagnostic)
PEER = '12D3KooW' + 'A' * 44
ADDRESS = f'/ip4/192.168.1.2/tcp/42931/p2p/{PEER}'


def name(value):
    return b''.join(bytes([len(part)]) + part.encode() for part in value.rstrip('.').split('.')) + b'\0'


def record(owner, kind, data, ttl=60):
    return owner + struct.pack('!HHIH', kind, 1, ttl, len(data)) + data


def response(address=ADDRESS, compressed=False, service=diagnostic.SERVICE, ttl=60):
    service_name = name(service)
    target = name('peer-instance.')
    ptr = record(service_name, 12, target, ttl)
    # The TXT owner uses a pointer to the earlier PTR target, as real DNS may.
    owner = struct.pack('!H', 0xc000 | (12 + len(service_name) + 10)) if compressed else target
    value = ('dnsaddr=' + address).encode()
    txt = record(owner, 16, bytes([len(value)]) + value, ttl)
    return struct.pack('!6H', 0, 0x8400, 0, 1, 0, 1) + ptr + txt


class DiscoveryDiagnosticsTests(unittest.TestCase):
    def test_libp2p_ptr_chain_accepts_plain_and_compressed_names(self):
        for compressed in (False, True):
            self.assertEqual(diagnostic.parse_response(response(compressed=compressed)), [ADDRESS])

    def test_unrelated_services_and_unlinked_txt_are_not_peers(self):
        self.assertEqual(diagnostic.parse_response(response(service='_printer._tcp.local.')), [])
        value = ('dnsaddr=' + ADDRESS).encode()
        packet = struct.pack('!6H', 0, 0x8400, 0, 1, 0, 0) + record(name('unrelated.'), 16, bytes([len(value)]) + value)
        self.assertEqual(diagnostic.parse_response(packet), [])
        self.assertEqual(diagnostic.parse_response(response(ttl=0)), [])

    def test_only_bounded_direct_tcp_addresses_are_reported(self):
        for bad in (ADDRESS.replace('/tcp/42931/', '/tcp/0/'),
                    ADDRESS.replace('/tcp/42931/', '/tcp/65536/'),
                    ADDRESS.replace('/ip4/192.168.1.2', '/ip4/not-an-ip'),
                    ADDRESS.replace('/ip4/', '/ip6/'), ADDRESS + '/p2p/' + PEER,
                    ADDRESS + '\n', ADDRESS.replace('/tcp/', '/udp/'),
                    ADDRESS.replace('/ip4/192.168.1.2', '/ip6/fe80::2%bad\ninterface')):
            self.assertEqual(diagnostic.parse_response(response(bad)), [])
        ipv6 = ADDRESS.replace('/ip4/192.168.1.2', '/ip6/fd00::2')
        self.assertEqual(diagnostic.parse_response(response(ipv6)), [ipv6])

    def test_truncation_bad_lengths_and_compression_cycles_fail_closed(self):
        packet = response(compressed=True)
        for length in range(len(packet)):
            with self.assertRaises(ValueError):
                diagnostic.parse_response(packet[:length])
        for malformed in (b'\xc0\x00', b'\xc0\xff', b'\x40', b'\x03ab', b'\x01\xff\0'):
            with self.assertRaises((ValueError, UnicodeError)):
                diagnostic.dns_name(malformed, 0)
        with self.assertRaises(ValueError):
            diagnostic.parse_response(b'\0' * (diagnostic.MAX_PACKET + 1))

    def test_queries_and_excessive_or_unsupported_responses_are_ignored_or_rejected(self):
        self.assertEqual(diagnostic.parse_response(struct.pack('!6H', 0, 0, 1, 0, 0, 0)), [])
        for header in ((0, 0x8400, 33, 0, 0, 0), (0, 0x8400, 0, 257, 0, 0),
                       (0, 0x8403, 0, 0, 0, 0), (0, 0x8600, 0, 0, 0, 0)):
            with self.assertRaises(ValueError):
                diagnostic.parse_response(struct.pack('!6H', *header))

    def test_interpretation_distinguishes_socket_failure_absence_and_observation(self):
        self.assertIn('No discovery query could be sent', diagnostic.interpretation(0, [], ['bind error']))
        absent = diagnostic.interpretation(1, [], [])
        self.assertIn('cannot distinguish', absent)
        self.assertNotIn('blocked', absent)
        self.assertIn('announcements received', diagnostic.interpretation(1, [ADDRESS], []))
        self.assertIn('Some interfaces also failed', diagnostic.interpretation(1, [], ['send error']))

    def test_socket_creation_failure_is_reported_without_crashing(self):
        with patch.object(diagnostic.socket, 'socket', side_effect=PermissionError('socket denied')):
            sent, packets, endpoints, sources, errors = diagnostic.probe([('wifi', '192.168.1.2')], 0)
        self.assertEqual((sent, packets, endpoints, sources), (0, 0, [], []))
        self.assertEqual(errors, ['wifi (192.168.1.2): socket denied'])

    def test_probe_accepts_libp2p_replies_from_ephemeral_source_ports(self):
        for source_port in (5353, 41000):
            with self.subTest(source_port=source_port):
                stream = Mock()
                stream.recvfrom.return_value = (response(), ('192.168.1.3', source_port))
                with patch.object(diagnostic.socket, 'socket', return_value=stream), \
                     patch.object(diagnostic.select, 'select', return_value=([stream], [], [])), \
                     patch.object(diagnostic.time, 'monotonic', side_effect=[0, 0, 0, 2]):
                    result = diagnostic.probe([('wifi', '192.168.1.2')], 1)
                self.assertEqual(result, (1, 1, [ADDRESS], ['192.168.1.3'], []))
                stream.close.assert_called_once()


if __name__ == '__main__':
    unittest.main()
