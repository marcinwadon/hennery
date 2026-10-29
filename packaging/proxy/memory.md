# Memory plug-in behind HTTPS

`hennery-memory serve` speaks plaintext HTTP. It never terminates TLS, and a
`https://` public URL from `ui-init` does not change that. Remote browsers and
MCP clients reach it only through a TLS reverse proxy on the same machine.

1. **Loopback backend.** Keep the default `--listen 127.0.0.1:7332`, or another
   loopback address. Never publish the plaintext port, on a LAN or in a
   container port mapping. Run the service as its own OS user with a private
   `--data-dir`; agents must not be able to read its databases or keys.
2. **Dedicated hostname.** Give the memory service its own DNS name and
   certificate, separate from hennery's UI and any other application
   (`ui-init --public-url https://memory.example`). The UI refuses IP-address
   origins, and cookies do not separate ports. `POST /mcp` uses the same
   hostname: it accepts only bearer credentials and refuses any request
   carrying `Origin` or `Cookie`.
3. **Proxy.** Start from [Caddyfile](Caddyfile) or [nginx.conf](nginx.conf)
   with `@BACKEND_PORT@` set to the memory port. Both overwrite every incoming
   `Hennery-Client-IP` with the proxy's own TCP peer, including duplicates, and
   ignore `Forwarded`/`X-Forwarded-For`. To serve the collector and memory on
   one HTTPS port, give each hostname its own site (Caddy) or `server` block
   with its own `server_name` and certificate (nginx), each keeping the
   template's directives. The fixture tests run the single-site templates
   against the collector, not this two-hostname variant: check the rendered
   configuration on a disposable instance first.
4. **Trusted attribution.** Start with
   `serve --proxy-client-ip-peer 127.0.0.1`, naming only the proxy's exact
   backend peer: repeat the flag for at most eight unicast addresses; no CIDR,
   hostname or port. Without the flag the header is ignored, so every client
   behind the proxy shares one password, passkey and step-up budget: still
   safe, but one client's failures lock out the rest. A trusted peer that sends
   a duplicate or malformed header gets 400; one that omits it is keyed by its
   own address. Trust is only as narrow as backend reachability: any local
   process that can reach the loopback port from a trusted address can choose
   its budget key. The address keys budgets only and never authenticates.
5. **Limits.** The nginx template caps each address at 20 connections and
   100 requests/s (burst 200). Caddy has no per-client limiter: add firewall or
   ingress caps before public exposure. Backend limits do not bound TLS
   handshakes. The service itself bounds MCP bodies at 128 KiB read within
   5 s, responses at 16 KiB, the management UI at four concurrent requests
   per pool, and refuses excess work with 503.

Generated with Claude AI — please review before distribution.
