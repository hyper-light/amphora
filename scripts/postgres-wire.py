"""A client of PostgreSQL's wire protocol, for the scripts that take a real server's answers: a server that asks for
no password, at protocol 3.0."""
import socket
import struct


class Client:
    def __init__(self, host, port, database):
        self.sock = socket.create_connection((host, port))
        body = struct.pack("!i", 196608) + b"user\0postgres\0database\0%s\0\0" % database.encode()
        self.sock.sendall(struct.pack("!i", len(body) + 4) + body)
        while True:
            kind, body = self.read()
            if kind == b"R" and struct.unpack("!i", body[:4])[0] != 0:
                raise SystemExit("the server asked for a password")
            if kind == b"Z":
                return

    def read(self):
        head = self.exact(5)
        length = struct.unpack("!i", head[1:])[0]
        return head[:1], self.exact(length - 4)

    def exact(self, n):
        data = b""
        while len(data) < n:
            more = self.sock.recv(n - len(data))
            if not more:
                raise SystemExit("the server closed the connection")
            data += more
        return data

    def query(self, sql):
        body = sql.encode() + b"\0"
        self.sock.sendall(b"Q" + struct.pack("!i", len(body) + 4) + body)
        messages = []
        while True:
            kind, body = self.read()
            messages.append((kind.decode(), body.hex()))
            if kind == b"Z":
                return messages
