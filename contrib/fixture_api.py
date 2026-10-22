#!/usr/bin/env python3
"""A stand-in Cloudflare v4 API, for driving the UI without a real account.

Serves enough of the endpoints Tunnel Manager calls to populate every page:
healthy / degraded / down / inactive tunnels, a locally configured one, a
handful of hostnames with correct, missing, unproxied and conflicting DNS, and
two zones.

Run:  python3 fixture_api.py 8787
Then: TUNNEL_MANAGER_API_BASE=http://127.0.0.1:8787/client/v4 cargo run
"""
import json
import re
import sys
from http.server import BaseHTTPRequestHandler, HTTPServer
from urllib.parse import urlparse, parse_qs

ACCOUNT = "0123456789abcdef0123456789abcdef"
ZONE_A = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
ZONE_B = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"

T1 = "f70ff985-a4ef-4643-bbbc-4a0ed4fc8415"
T2 = "c1744f8b-faa1-48a4-9e5c-02ac921467fa"
T3 = "2bdc4f11-8e91-4c2a-9f77-1d5b3a9c6e01"
T4 = "9a1c2e3d-4f56-4789-abcd-0123456789ab"
T5 = "77aa88bb-99cc-4dde-8ff0-112233445566"


def envelope(result, info=None):
    body = {"success": True, "errors": [], "messages": [], "result": result}
    if info is not None:
        body["result_info"] = info
    return body


def conn(colo, client_id, pending=False):
    return {
        "colo_name": colo,
        "uuid": f"{client_id}-{colo}",
        "id": f"{client_id}-{colo}",
        "is_pending_reconnect": pending,
        "origin_ip": "203.0.113.42",
        "opened_at": "2026-08-20T09:14:02.101642Z",
        "client_id": client_id,
        "client_version": "2026.6.1",
    }


TUNNELS = [
    {
        "id": T1,
        "account_tag": ACCOUNT,
        "created_at": "2025-11-02T09:12:31.534395Z",
        "deleted_at": None,
        "name": "production web",
        "connections": [conn("FRA", "conn-a"), conn("AMS", "conn-a"),
                        conn("FRA", "conn-a"), conn("AMS", "conn-a")],
        "conns_active_at": "2026-08-22T08:41:12.004721Z",
        "conns_inactive_at": None,
        "tun_type": "cfd_tunnel",
        "metadata": {},
        "status": "healthy",
        "remote_config": True,
        "config_src": "cloudflare",
    },
    {
        "id": T2,
        "account_tag": ACCOUNT,
        "created_at": "2026-03-14T15:09:26.291225Z",
        "deleted_at": None,
        "name": "staging api",
        "connections": [conn("LHR", "conn-b"), conn("LHR", "conn-b", pending=True)],
        "conns_active_at": "2026-08-22T07:02:00.000000Z",
        "conns_inactive_at": None,
        "tun_type": "cfd_tunnel",
        "metadata": {},
        "status": "degraded",
        "remote_config": True,
        "config_src": "cloudflare",
    },
    {
        "id": T3,
        "account_tag": ACCOUNT,
        "created_at": "2026-01-08T11:45:00.000000Z",
        "deleted_at": None,
        "name": "office jump host",
        "connections": [],
        "conns_active_at": None,
        "conns_inactive_at": "2026-08-19T22:15:00.000000Z",
        "tun_type": "cfd_tunnel",
        "metadata": {},
        "status": "down",
        "remote_config": True,
        "config_src": "cloudflare",
    },
    {
        "id": T4,
        "account_tag": ACCOUNT,
        "created_at": "2026-08-21T18:00:00.000000Z",
        "deleted_at": None,
        "name": "new tunnel",
        "connections": [],
        "conns_active_at": None,
        "conns_inactive_at": "2026-08-21T18:00:00.000000Z",
        "tun_type": "cfd_tunnel",
        "metadata": {},
        "status": "inactive",
        "remote_config": True,
        "config_src": "cloudflare",
    },
    {
        "id": T5,
        "account_tag": ACCOUNT,
        "created_at": "2024-06-30T08:00:00.000000Z",
        "deleted_at": None,
        "name": "legacy config-file tunnel",
        "connections": [conn("CDG", "conn-e")],
        "conns_active_at": "2026-08-22T06:00:00.000000Z",
        "conns_inactive_at": None,
        "tun_type": "cfd_tunnel",
        "metadata": {},
        "status": "healthy",
        "remote_config": False,
        "config_src": "local",
    },
]

CONFIGS = {
    T1: {
        "ingress": [
            {"hostname": "app.example.com", "service": "http://localhost:8080",
             "originRequest": {"connectTimeout": 30, "noTLSVerify": False,
                               "futureField": "preserved"}},
            {"hostname": "api.example.com", "path": "/v2",
             "service": "https://10.0.0.5:8443"},
            {"hostname": "files.example.com", "service": "http://localhost:9000"},
            {"service": "http_status:404"},
        ],
        "warp-routing": {"enabled": False},
    },
    T2: {
        "ingress": [
            {"hostname": "staging.example.net", "service": "http://localhost:3000"},
            {"hostname": "ssh.example.net", "service": "ssh://localhost:22"},
            {"service": "http_status:404"},
        ],
    },
    T3: {
        "ingress": [
            {"hostname": "jump.example.net", "service": "tcp://localhost:2222"},
            {"service": "http_status:404"},
        ],
    },
    T4: {"ingress": [{"service": "http_status:404"}]},
}

DNS = {
    "app.example.com": [
        {"id": "rec-app", "zone_id": ZONE_A, "name": "app.example.com",
         "type": "CNAME", "content": f"{T1}.cfargotunnel.com", "proxied": True,
         "ttl": 1, "comment": "Managed by Tunnel Manager"}
    ],
    # Present but grey-clouded: repairable.
    "api.example.com": [
        {"id": "rec-api", "zone_id": ZONE_A, "name": "api.example.com",
         "type": "CNAME", "content": f"{T1}.cfargotunnel.com", "proxied": False,
         "ttl": 1}
    ],
    # Nothing at all: repairable by creating the record.
    "files.example.com": [],
    # Somebody else's A record: a conflict this application will not touch.
    "staging.example.net": [
        {"id": "rec-staging", "zone_id": ZONE_B, "name": "staging.example.net",
         "type": "A", "content": "203.0.113.10", "proxied": True, "ttl": 1}
    ],
    "ssh.example.net": [
        {"id": "rec-ssh", "zone_id": ZONE_B, "name": "ssh.example.net",
         "type": "CNAME", "content": f"{T2}.cfargotunnel.com", "proxied": True,
         "ttl": 1}
    ],
    "jump.example.net": [],
}

ZONES = [
    {"id": ZONE_A, "name": "example.com", "status": "active", "paused": False,
     "account": {"id": ACCOUNT, "name": "Acme Engineering"},
     "permissions": ["#zone:read", "#dns_records:edit"]},
    {"id": ZONE_B, "name": "example.net", "status": "pending", "paused": False,
     "account": {"id": ACCOUNT, "name": "Acme Engineering"},
     "permissions": ["#zone:read", "#dns_records:edit"]},
]


class Handler(BaseHTTPRequestHandler):
    def log_message(self, *args):
        sys.stderr.write("%s - %s\n" % (self.command, self.path))

    def reply(self, body, status=200):
        payload = json.dumps(body).encode()
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(payload)))
        self.end_headers()
        self.wfile.write(payload)

    def route(self):
        parsed = urlparse(self.path)
        path = parsed.path.replace("/client/v4", "", 1)
        query = parse_qs(parsed.query)
        return path, query

    def do_GET(self):
        path, query = self.route()

        if path == "/user/tokens/verify":
            return self.reply(envelope(
                {"id": "100bf38cc8393103870917dd535e0628", "status": "active"}))

        if path == "/accounts":
            return self.reply(envelope(
                [{"id": ACCOUNT, "name": "Acme Engineering", "type": "standard"}],
                {"page": 1, "per_page": 50, "count": 1, "total_count": 1}))

        if path == "/zones":
            return self.reply(envelope(
                ZONES,
                {"page": 1, "per_page": 50, "count": len(ZONES),
                 "total_count": len(ZONES), "total_pages": 1}))

        if re.fullmatch(r"/accounts/[^/]+/cfd_tunnel", path):
            return self.reply(envelope(
                TUNNELS,
                {"page": 1, "per_page": 50, "count": len(TUNNELS),
                 "total_count": len(TUNNELS)}))

        match = re.fullmatch(r"/accounts/[^/]+/cfd_tunnel/([^/]+)/configurations", path)
        if match:
            tunnel = match.group(1)
            return self.reply(envelope({
                "tunnel_id": tunnel, "account_id": ACCOUNT, "version": 7,
                "created_at": "2026-08-01T00:00:00Z", "source": "cloudflare",
                "config": CONFIGS.get(tunnel),
            }))

        match = re.fullmatch(r"/accounts/[^/]+/cfd_tunnel/([^/]+)/connections", path)
        if match:
            tunnel = match.group(1)
            found = next((t for t in TUNNELS if t["id"] == tunnel), None)
            if not found or not found["connections"]:
                return self.reply(envelope([]))
            by_client = {}
            for c in found["connections"]:
                by_client.setdefault(c["client_id"], []).append(c)
            return self.reply(envelope([
                {"id": client, "features": ["ha-origin"], "version": "2026.6.1",
                 "arch": "linux_amd64", "run_at": "2026-08-22T06:00:00Z",
                 "config_version": 7, "conns": conns}
                for client, conns in by_client.items()
            ]))

        match = re.fullmatch(r"/accounts/[^/]+/cfd_tunnel/([^/]+)/token", path)
        if match:
            return self.reply(envelope("eyJhIjoiZml4dHVyZSIsInQiOiJ0b2tlbiJ9"))

        match = re.fullmatch(r"/accounts/[^/]+/cfd_tunnel/([^/]+)", path)
        if match:
            tunnel = next((t for t in TUNNELS if t["id"] == match.group(1)), None)
            return self.reply(envelope(tunnel))

        match = re.fullmatch(r"/zones/([^/]+)/dns_records", path)
        if match:
            name = (query.get("name.exact") or query.get("name") or [""])[0]
            records = DNS.get(name, [])
            return self.reply(envelope(
                records,
                {"page": 1, "per_page": 50, "count": len(records),
                 "total_count": len(records), "total_pages": 1}))

        return self.reply({"success": False,
                           "errors": [{"code": 7003, "message": "No route for the URI"}],
                           "messages": [], "result": None}, 404)

    def read_json_body(self):
        # Loopback-only fixture, but a missing or absurd length must not
        # allocate: cap at 1 MiB and reject the request above it.
        try:
            length = int(self.headers.get("Content-Length") or 0)
        except ValueError:
            length = 0
        if length < 0 or length > 1024 * 1024:
            return None
        raw = self.rfile.read(length) if length else b"{}"
        try:
            return json.loads(raw or b"{}")
        except ValueError:
            return {}

    def do_POST(self):
        path, _ = self.route()
        body = self.read_json_body()
        if body is None:
            return self.reply(envelope(None), 413)

        if re.fullmatch(r"/accounts/[^/]+/cfd_tunnel", path):
            new = {
                "id": "11112222-3333-4444-5555-666677778888",
                "account_tag": ACCOUNT,
                "created_at": "2026-08-22T16:00:00Z",
                "deleted_at": None,
                "name": body.get("name", "unnamed"),
                "connections": [], "conns_active_at": None,
                "conns_inactive_at": "2026-08-22T16:00:00Z",
                "tun_type": "cfd_tunnel", "metadata": {},
                "status": "inactive", "remote_config": True,
                "token": "eyJhIjoiZml4dHVyZSIsInQiOiJuZXcifQ==",
            }
            TUNNELS.append(new)
            CONFIGS[new["id"]] = {"ingress": [{"service": "http_status:404"}]}
            return self.reply(envelope(new))

        match = re.fullmatch(r"/zones/([^/]+)/dns_records", path)
        if match:
            record = {"id": "rec-new", "zone_id": match.group(1),
                      "name": body.get("name"), "type": body.get("type"),
                      "content": body.get("content"),
                      "proxied": body.get("proxied", False), "ttl": 1,
                      "comment": body.get("comment")}
            DNS.setdefault(body.get("name", ""), []).append(record)
            return self.reply(envelope(record))

        return self.reply(envelope(None))

    def do_PUT(self):
        path, _ = self.route()
        body = self.read_json_body()
        if body is None:
            return self.reply(envelope(None), 413)

        match = re.fullmatch(r"/accounts/[^/]+/cfd_tunnel/([^/]+)/configurations", path)
        if match:
            CONFIGS[match.group(1)] = body.get("config", {})
            return self.reply(envelope({
                "tunnel_id": match.group(1), "version": 8, "source": "cloudflare",
                "config": body.get("config", {})}))

        return self.reply(envelope(None))

    def do_PATCH(self):
        path, _ = self.route()
        body = self.read_json_body()
        if body is None:
            return self.reply(envelope(None), 413)

        match = re.fullmatch(r"/accounts/[^/]+/cfd_tunnel/([^/]+)", path)
        if match:
            for tunnel in TUNNELS:
                if tunnel["id"] == match.group(1):
                    tunnel["name"] = body.get("name", tunnel["name"])
                    return self.reply(envelope(tunnel))

        match = re.fullmatch(r"/zones/([^/]+)/dns_records/([^/]+)", path)
        if match:
            for records in DNS.values():
                for record in records:
                    if record["id"] == match.group(2):
                        record.update({k: v for k, v in body.items() if v is not None})
                        return self.reply(envelope(record))

        return self.reply(envelope(None))

    def do_DELETE(self):
        path, _ = self.route()

        match = re.fullmatch(r"/accounts/[^/]+/cfd_tunnel/([^/]+)", path)
        if match:
            TUNNELS[:] = [t for t in TUNNELS if t["id"] != match.group(1)]
            return self.reply(envelope({"id": match.group(1)}))

        match = re.fullmatch(r"/zones/([^/]+)/dns_records/([^/]+)", path)
        if match:
            for name, records in DNS.items():
                DNS[name] = [r for r in records if r["id"] != match.group(2)]
            return self.reply({"result": {"id": match.group(2)}})

        return self.reply(envelope(None))


if __name__ == "__main__":
    port = int(sys.argv[1]) if len(sys.argv) > 1 else 8787
    print(f"fixture Cloudflare API on http://127.0.0.1:{port}/client/v4")
    HTTPServer(("127.0.0.1", port), Handler).serve_forever()
