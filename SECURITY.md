# Security

## Reporting a vulnerability

Report it privately through GitHub: on
[JavenKazebee/capture-room](https://github.com/JavenKazebee/capture-room), open the
**Security** tab and choose **Report a vulnerability**
([direct link](https://github.com/JavenKazebee/capture-room/security/advisories/new)).

Please don't open a public issue, discussion or pull request for it.

Include:

- the version (menu bar, Settings › Version, or `capture-room --version`) and OS;
- what an attacker can do, and from where (same machine, the local network, the internet);
- steps or a proof of concept to reproduce it;
- any fix or mitigation you have in mind.

## Security model

Capture Room is meant for a production's own network, not the open internet.

- **Access token.** One shared secret per cluster, made at random the first time a node
  starts, so no node is ever open. Every `/api` route and WebSocket needs it; only the
  UI assets, sign-in, node identity and pairing (`/api/v1/auth/…`) and WHEP playback
  (`/whep`) are open. Browsers sign in once and get an HttpOnly, `SameSite=Strict` cookie.
- **TLS.** Each node has its own key and a self-signed certificate. Plain HTTP is answered
  only from the same machine and for `/whep`; anything else is redirected to HTTPS.
- **Nodes.** A controller pins each node's key when it's approved and talks to it only
  over HTTPS with that key. Adding a node takes its pairing code (8 characters, replaced
  after use or 5 wrong tries), which lets the controller hand it the cluster's token.
- **Local access.** Whoever is at a node's machine is trusted: the menu bar opens the UI
  signed in and can show the token, and one-time sign-in links work only from that
  machine.
- **WHIP.** Each WHIP source has its own port and bearer token, checked by a proxy in
  front of GStreamer's WHIP server.
- **Updates.** A node installs only releases signed with the project's minisign key,
  which is compiled in.

## What to expect

Capture Room is alpha software maintained by one person, so there's no response-time
guarantee. Reports are taken seriously: you'll get an answer, a fix in a release when it's
confirmed, and credit in the advisory if you want it.

## Supported versions

Only the latest release gets fixes. Installed copies update themselves; update before
reporting if you can.
