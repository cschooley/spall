---
name: scope-review
description: Decide which hosts seen in traffic belong to the engagement, and say why, as proposals for a human.
---
# Scope review

1. `hosts --unknown` lists hosts nobody has decided on. Take them one at a time.
2. For each, `filter host:<name>` and `open` one or two exchanges. Look at:
   - who links to it (the Referer and the page that loaded it),
   - what it serves (an API the app calls, a login, static assets, a tracker),
   - its name next to the in-scope hosts (`api.client.test` beside `client.test`).
3. Sort them:
   - **Belongs**: the client's own app, API, auth or admin hosts.
   - **Stays out**: ad networks, analytics, CDNs and SaaS the client doesn't own.
     Testing those is testing someone else.
   - **Ask**: anything you can't tell. Say what would settle it.
4. Put the list in your report: host, verdict, one line of reasoning, the
   exchange id that shows it. Only the tester changes scope.
