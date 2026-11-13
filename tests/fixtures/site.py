"""A small site for bitti's own tests: `/flaky` fails its first visit
and passes after, `/late` shows its text after `?ms=`, `/m1` has what the
newer expect subjects read, `/login` signs in (counted at `/logins`), and
`/note?x=` records what `before/after all` did (`/notes`); `/api/…` is the
API `api.bt` tests with no browser."""
import email
import json
import socketserver
import sys
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from urllib.parse import parse_qs, urlparse

VISITS = {"flaky": 0}
LOGINS = [0]
NOTES = []
ORDERS = [0]
JOBS = []

M1 = """
<div data-testid=toast>Saved order 7</div>
<span data-testid=badge style="color: rgb(0, 128, 0)">ok</span>
<input aria-label=Notes>
<textarea aria-label=Filled>x</textarea>
<button id=order onclick="fetch('/api/order', {method: 'POST', body: '{}'}).then((r) => r.json()).then((j) => { document.title = j.id; })">Order</button>
<a id=dl href=/report.csv>Report</a>
<script>console.log('m1 ready')</script>
"""

LOGIN = """
<h1>Sign in</h1>
<input aria-label=Password type=password id=pw>
<button onclick="fetch('/session', {method: 'POST', body: pw.value}).then((r) => { if (r.ok) location = '/account'; else document.body.append('wrong password'); })">Sign in</button>
"""

# `/form` is a form; with `?auto=1` it uses itself, as a person would, for
# `bitti record` with nobody at the browser (`?auto=2`: after 1.5 s, for
# `record --extend`, which runs the scenario first).
FORM = """
<h1>Order</h1>
<label>Email <input id=email></label>
<label><input type=checkbox id=gift> Gift wrap</label>
<label>Size <select id=size><option>Small</option><option>Large</option></select></label>
<x-tip id=tip></x-tip>
<a href=/done id=go>Place order</a>
<script>
// A web component's icon button: no name, inside a shadow root.
customElements.define('x-tip', class extends HTMLElement { connectedCallback() {
  if (!this.shadowRoot) this.attachShadow({mode: 'open'}).innerHTML = '<button><svg width=12 height=12></svg></button>';
  this.shadowRoot.querySelector('button').onclick = () => { this.dataset.open = '1'; };
} });
const auto = new URLSearchParams(location.search).get('auto');
if (auto) setTimeout(() => {
  email.click(); email.value = 'ada@example.com'; email.dispatchEvent(new Event('change', {bubbles: true}));
  gift.click();
  tip.shadowRoot.querySelector('button').click();
  size.value = 'Large'; size.dispatchEvent(new Event('change', {bubbles: true}));
  setTimeout(() => go.click(), 100);
}, auto === '2' ? 1500 : 300);
</script>
"""

# `/bar`: a consent bar over the Buy button; its Accept takes it off the
# page (`?auto=1`: Accept, then Buy, for `bitti record`).
BAR = """
<h1>Shop</h1>
<button onclick="document.body.append('Bought')" style="margin-top:80vh">Buy now</button>
<div id=bar style="position:fixed;left:0;right:0;bottom:0;height:40vh;background:#eee">Cookies
  <button onclick="bar.remove()">Accept</button></div>
<script>
if (new URLSearchParams(location.search).get('auto')) setTimeout(() => {
  bar.querySelector('button').click();
  setTimeout(() => document.querySelector('button').click(), 100);
}, 300);
</script>
"""

class Handler(BaseHTTPRequestHandler):
    def log_message(self, *a):
        pass

    def page(self, body):
        data = f"<!doctype html><html><head><meta charset=utf-8><title>t</title></head><body>{body}</body></html>".encode()
        self.send_response(200)
        self.send_header("Content-Type", "text/html; charset=utf-8")
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def raw(self, data, kind="text/plain", headers=()):
        data = data.encode() if isinstance(data, str) else data
        self.send_response(200)
        self.send_header("Content-Type", kind)
        self.send_header("Content-Length", str(len(data)))
        for k, v in headers:
            self.send_header(k, v)
        self.end_headers()
        self.wfile.write(data)

    def answer(self, status, text):
        data = text.encode()
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def sid(self):
        return next((c.split("=", 1)[1] for c in (self.headers.get("Cookie") or "").split("; ") if c.startswith("sid=")), None)

    def do_POST(self):
        u = urlparse(self.path)
        raw = self.rfile.read(int(self.headers.get("Content-Length") or 0))
        body = raw.decode(errors="replace")
        # The API a scenario with no browser tests (`api.bt`).
        if u.path == "/api/login":
            if self.headers.get("Content-Type") != "application/x-www-form-urlencoded":
                return self.answer(415, '{"error": "send a form"}')
            form = parse_qs(body)
            if form.get("user") == ["ada"] and form.get("password") == ["hunter22"]:
                LOGINS[0] += 1
                return self.raw('{"ok": true}', "application/json", [("Set-Cookie", "sid=api; Path=/api; HttpOnly")])
            return self.answer(403, '{"error": "wrong password"}')
        if u.path == "/graphql":
            # A GraphQL endpoint: errors come back with a 200, as the spec has it.
            req = json.loads(body or "{}")
            q, v = req.get("query", ""), req.get("variables") or {}
            if "product" in q and v.get("id") == "p1":
                return self.answer(200, json.dumps({"data": {"product": {"id": "p1", "name": "Trail shoe", "price": 89.5}}}))
            return self.answer(200, json.dumps({"data": None, "errors": [{"message": "Cannot query field 'nope' on type 'Query'"}]}))
        if u.path == "/api/upload":
            msg = email.message_from_bytes(b"Content-Type: " + self.headers.get("Content-Type", "").encode() + b"\r\n\r\n" + raw)
            fields, files = {}, {}
            for part in msg.walk():
                name = part.get_param("name", header="content-disposition")
                if not name:
                    continue
                data = part.get_payload(decode=True) or b""
                if part.get_filename():
                    files[name] = {"filename": part.get_filename(), "size": len(data), "type": part.get_content_type()}
                else:
                    fields[name] = data.decode()
            return self.answer(200, json.dumps({"fields": fields, "files": files}))
        if u.path == "/api/jobs":
            JOBS.append(0)
            return self.answer(202, json.dumps({"id": str(len(JOBS) - 1), "status": "pending"}))
        if u.path == "/api/order":
            ORDERS[0] += 1
            return self.raw('{"id": "ORD-7", "items": [{"sku": "sku-1", "qty": 2}], "total": 36}', "application/json")
        if u.path == "/api/users":
            if self.headers.get("Content-Type") != "application/json":
                return self.answer(415, '{"error": "send JSON"}')
            got = json.loads(body)
            return self.answer(201, json.dumps({"id": "u-1", "email": got["email"], "role": got["role"]}))
        if u.path == "/session":
            if body == "hunter22":
                LOGINS[0] += 1
                return self.raw("ok", headers=[("Set-Cookie", "sid=abc; Path=/")])
            self.send_response(403)
            self.send_header("Content-Length", "0")
            self.end_headers()
            return
        self.send_response(404)
        self.send_header("Content-Length", "0")
        self.end_headers()

    def do_GET(self):
        u = urlparse(self.path)
        q = parse_qs(u.query)
        if u.path == "/form":
            return self.page(FORM)
        if u.path == "/bar":
            return self.page(BAR)
        if u.path == "/done":
            return self.page("<h1>Thanks</h1>")
        if u.path == "/framed":
            # A button inside a frame whose name a test misspells.
            return self.page('<iframe srcdoc="<button>Subscibe</button>"></iframe>')
        if u.path == "/clicks":
            # A button whose handler keeps the page busy for 250 ms, and one that answers at once.
            return self.page('<button id=slow onclick="let t=performance.now();while(performance.now()-t<250){}">Slow</button>'
                             '<button id=quick onclick="this.textContent=\'Done\'">Quick</button>')
        if u.path == "/a11y":
            # An image without alt and a button without a name, outside #ok.
            return self.page('<main><h1>Gallery</h1><section id=ok><p>Fine here.</p></section>'
                             '<img src="/report.csv" width=10 height=10><button><svg width=8 height=8></svg></button></main>')
        if u.path == "/m1":
            return self.page(M1)
        if u.path == "/api/x":
            return self.raw("real")
        if u.path == "/report.csv":
            return self.raw("id,name\n1,Ada\n", "text/csv", [("Content-Disposition", 'attachment; filename="report-2026.csv"')])
        if u.path == "/login":
            return self.page(LOGIN)
        if u.path == "/api/profile":
            if self.sid() != "api":
                return self.answer(401, '{"error": "signed out"}')
            return self.answer(200, json.dumps({"id": 7, "email": "ada@example.com", "role": "shopper",
                                                "tags": ["a", "b"], "address": {"city": "Oslo", "zip": "0150"}}))
        if u.path == "/api/search":
            return self.answer(200, json.dumps({"q": q.get("q", [""])[0], "n": int(q.get("n", ["0"])[0])}))
        if u.path.startswith("/api/jobs/"):
            i = int(u.path.rsplit("/", 1)[1])
            JOBS[i] += 1
            return self.answer(200, json.dumps({"id": str(i), "status": "done" if JOBS[i] >= 3 else "pending"}))
        if u.path == "/api/basic":
            ok = self.headers.get("Authorization") == "Basic YWRhOmh1bnRlcjIy"  # ada:hunter22
            return self.answer(200, '{"user": "ada"}') if ok else self.answer(401, '{"error": "who?"}')
        if u.path == "/api/token":
            ok = self.headers.get("Authorization") == "Bearer t0k"
            return self.answer(200, '{"token": "ok"}') if ok else self.answer(401, '{"error": "no token"}')
        if u.path == "/api/items":
            return self.answer(200, '[{"sku": "a", "price": 9.99}, {"sku": "b", "price": "5"}]')
        if u.path == "/api/me":
            sid = next((c.split("=", 1)[1] for c in (self.headers.get("Cookie") or "").split("; ") if c.startswith("sid=")), None)
            return self.answer(200, json.dumps({"sid": sid})) if sid else self.answer(401, '{"error": "signed out"}')
        if u.path == "/api/hop":
            self.send_response(302)
            self.send_header("Location", "/api/me")
            self.send_header("Set-Cookie", "sid=hop; Path=/")
            self.send_header("Content-Length", "0")
            self.end_headers()
            return
        if u.path == "/api/fail":
            return self.answer(500, "db down")
        if u.path == "/account":
            signed = "sid=abc" in (self.headers.get("Cookie") or "")
            return self.page("<h1>Account</h1><p>Signed in</p>" if signed else "<h1>Sign in first</h1>")
        if u.path == "/logins":
            return self.raw(str(LOGINS[0]))
        if u.path == "/note":
            NOTES.append(q.get("x", [""])[0])
            return self.page("<p>noted</p>")
        if u.path == "/notes":
            return self.raw(",".join(NOTES))
        if u.path == "/flaky":
            VISITS["flaky"] += 1
            return self.page("<p>ok</p>" if VISITS["flaky"] > 1 else "<p>not yet</p>")
        if u.path == "/expiry":
            # Signed out after 15 idle minutes; today's date at the top.
            return self.page(
                "<p id=d></p><p id=s>Signed in</p><script>"
                "d.textContent = 'Today ' + new Date().toISOString().slice(0, 10);"
                "setTimeout(() => s.textContent = 'Session expired', 15 * 60 * 1000)</script>"
            )
        if u.path == "/late":
            ms = int(q.get("ms", ["300"])[0])
            return self.page(f"<p id=s>loading</p><script>setTimeout(() => s.textContent = 'ready', {ms})</script>")
        if u.path == "/api/n":
            time.sleep(0.05)
            data = b'{"n": 3}'
            self.send_response(200)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(data)))
            self.end_headers()
            return self.wfile.write(data)
        return self.page("<h1>Home</h1><a href=/late>Late</a>")


class Server(ThreadingHTTPServer):
    """A ThreadingHTTPServer that binds without looking up its host's name.

    HTTPServer.server_bind ends with socket.getfqdn(host), a reverse lookup
    whose answer nothing here reads. On GitHub's macOS runners it takes
    about 35 s, paid at every start.
    """

    def server_bind(self):
        socketserver.TCPServer.server_bind(self)
        self.server_name, self.server_port = self.server_address[:2]


if __name__ == "__main__":
    port = int(sys.argv[sys.argv.index("--port") + 1]) if "--port" in sys.argv else 0
    srv = Server(("127.0.0.1", port), Handler)
    print(f"LISTENING http://127.0.0.1:{srv.server_address[1]}", flush=True)
    try:
        srv.serve_forever()
    except KeyboardInterrupt:
        pass
    sys.exit(0)
