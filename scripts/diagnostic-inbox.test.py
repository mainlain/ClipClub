#!/usr/bin/env python3
import contextlib
import importlib.util
import io
import json
import os
from pathlib import Path
import stat
import tempfile
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import unittest
from unittest import mock

spec = importlib.util.spec_from_file_location('diagnostic_inbox', Path(__file__).with_name('diagnostic-inbox.py'))
inbox = importlib.util.module_from_spec(spec)
spec.loader.exec_module(inbox)
TOKEN = 'private-read-key-' + 'a' * 48
RID = 'a' * 32


class Server(BaseHTTPRequestHandler):
    calls = []
    mode = 'normal'

    def log_message(self, *_args):
        pass

    def do_GET(self):
        self.calls.append((self.path, self.headers.get('Authorization'), self.headers.get('User-Agent')))
        if self.path.startswith('/redirect'):
            self.send_response(302)
            self.send_header('Location', '/leaked-key')
            self.end_headers()
            return
        if self.headers.get('Authorization') != 'Bearer ' + TOKEN:
            self.send_response(401)
            self.end_headers()
            return
        if self.path.startswith('/v1/reports?'):
            if self.mode == 'bad-id':
                body = {'reports': [{'id': '../wallet'}], 'cursor': None}
            elif 'cursor=next' not in self.path:
                body = {'reports': [], 'cursor': 'next'}
            else:
                body = {'reports': [{'id': RID, 'device': 'Test Mac'}], 'cursor': None}
        else:
            body = {'id': RID, 'report': 'Test report\n', 'device': 'Test Mac'}
        self.send_response(200)
        self.send_header('Content-Type', 'application/json')
        self.end_headers()
        self.wfile.write(json.dumps(body).encode())


class Tests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.server = ThreadingHTTPServer(('127.0.0.1', 0), Server)
        cls.thread = threading.Thread(target=cls.server.serve_forever, daemon=True)
        cls.thread.start()
        cls.endpoint = 'http://127.0.0.1:' + str(cls.server.server_port)

    @classmethod
    def tearDownClass(cls):
        cls.server.shutdown()
        cls.server.server_close()
        cls.thread.join()

    def setUp(self):
        Server.mode = 'normal'
        Server.calls = []

    def test_endpoint_policy(self):
        self.assertEqual(inbox.endpoint_url('https://inbox.example/'), 'https://inbox.example')
        self.assertEqual(inbox.endpoint_url(self.endpoint), self.endpoint)
        for value in ('http://example.com', 'https://a:secret@example.com', 'https://a/prefix',
                      'https://a?token=secret', 'https://a#secret', 'file:///tmp/private', 'https://a:wrong'):
            with self.assertRaises(inbox.InboxError):
                inbox.endpoint_url(value)

    def test_private_config_and_atomic_no_replace(self):
        with tempfile.TemporaryDirectory() as folder:
            path = Path(folder) / 'config.json'
            inbox.atomic_private(path, b'{"endpoint":"https://a"}', replace=True)
            self.assertEqual(stat.S_IMODE(path.stat().st_mode), 0o600)
            self.assertEqual(inbox.read_config(path)['endpoint'], 'https://a')
            inbox.atomic_private(path, b'overwritten')
            self.assertEqual(path.read_text(), '{"endpoint":"https://a"}')
            path.chmod(0o644)
            with self.assertRaises(inbox.InboxError):
                inbox.read_config(path)

    def test_sparse_pagination_and_session_encoding(self):
        client = inbox.Client(self.endpoint, TOKEN)
        reports = list(client.reports('wifi & hotspot'))
        self.assertEqual([r['id'] for r in reports], [RID])
        self.assertEqual(len(Server.calls), 2)
        self.assertIn('session=wifi+%26+hotspot', Server.calls[0][0])
        self.assertTrue(all(call[1] == 'Bearer ' + TOKEN for call in Server.calls))
        self.assertTrue(all(call[2] == inbox.USER_AGENT for call in Server.calls))
        self.assertNotIn('Python-urllib', Server.calls[0][2])

    def test_pull_outputs_json_and_text_and_skips_existing(self):
        with tempfile.TemporaryDirectory() as folder:
            output = Path(folder)
            client = inbox.Client(self.endpoint, TOKEN)
            self.assertEqual(inbox.pull(client, output), (1, 0))
            self.assertEqual((output / (RID + '.txt')).read_text(), 'Test report\n')
            self.assertEqual(json.loads((output / (RID + '.json')).read_text())['id'], RID)
            self.assertEqual(stat.S_IMODE((output / (RID + '.txt')).stat().st_mode), 0o600)
            Server.calls = []
            self.assertEqual(inbox.pull(client, output), (0, 1))
            self.assertEqual(len(Server.calls), 2)

    def test_redirects_do_not_forward_read_key(self):
        client = inbox.Client(self.endpoint, TOKEN)
        with self.assertRaisesRegex(inbox.InboxError, 'HTTP 302'):
            client.get('/redirect')
        self.assertEqual(len(Server.calls), 1)

    def test_remote_path_traversal_and_local_symlinks_are_rejected(self):
        client = inbox.Client(self.endpoint, TOKEN)
        Server.mode = 'bad-id'
        with self.assertRaisesRegex(inbox.InboxError, 'invalid report ID'):
            list(client.reports())
        Server.mode = 'normal'
        with tempfile.TemporaryDirectory() as folder:
            output = Path(folder)
            target = output / 'wallet'
            target.write_text('keep private')
            (output / (RID + '.json')).symlink_to(target)
            with self.assertRaises(inbox.InboxError):
                inbox.pull(client, output)
            self.assertEqual(target.read_text(), 'keep private')

    def test_configure_hides_key_and_never_accepts_it_as_argument(self):
        with tempfile.TemporaryDirectory() as folder:
            config = Path(folder) / 'config.json'
            output = io.StringIO()
            with mock.patch.object(inbox.getpass, 'getpass', return_value=TOKEN), contextlib.redirect_stdout(output):
                self.assertEqual(inbox.main(['--config', str(config), '--endpoint', self.endpoint, 'configure']), 0)
            self.assertNotIn(TOKEN, output.getvalue())
            self.assertEqual(inbox.read_config(config)['read_token'], TOKEN)

    def test_http_errors_omit_keys_and_untrusted_body(self):
        client = inbox.Client(self.endpoint, 'wrong' * 10)
        with self.assertRaises(inbox.InboxError) as error:
            list(client.reports())
        self.assertEqual(str(error.exception), 'The inbox rejected the read key.')
        self.assertNotIn(TOKEN, str(error.exception))


if __name__ == '__main__':
    unittest.main()
