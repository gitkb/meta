"""Serve a fake release directory (tests/install.bats, tests/plugin_install.bats).

Usage: release_server.py <dir> <port-file>

Serves <dir> over HTTP on a free local port and writes the port to <port-file>.
A request for <path> answers with HTTP status N, and no file, when
<dir>/<path>.status contains N.
"""

import functools
import http.server
import pathlib
import socketserver
import sys


class Handler(http.server.SimpleHTTPRequestHandler):
    def do_GET(self):
        status = pathlib.Path(self.translate_path(self.path) + ".status")
        if status.is_file():
            self.send_error(int(status.read_text()))
        else:
            super().do_GET()


root, port_file = sys.argv[1], sys.argv[2]
# TCPServer, not HTTPServer: HTTPServer looks up the host's FQDN, which takes
# seconds on macOS runners
server = socketserver.TCPServer(
    ("127.0.0.1", 0), functools.partial(Handler, directory=root)
)
pathlib.Path(port_file).write_text(str(server.server_address[1]))
server.serve_forever()
