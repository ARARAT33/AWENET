# Site Hosting Reliability — 2026-11-15

This change closes three end-to-end hosting gaps found during review of the Phase 2 site-management integration.

## Restart-stable storage quotas

`LocalNodeStore` quotas are maximum-used-byte limits. On startup, the node now measures the existing content-addressed bytes in both the node store and site store before splitting newly available disk space.

The new limits are:

- node quota = existing node objects + 75% of currently free bytes;
- site quota = existing site objects + 25% of currently free bytes.

Checked arithmetic fails closed on overflow. Existing data therefore remains within its configured quota after restart, while newly available capacity keeps the existing 75/25 allocation policy. A zero quota for one store is allowed when the other store has existing data; the process only rejects startup when neither store has any allocatable capacity.

## Dedicated site origins

A request to a site-specific origin such as `<site-id>.localhost` is now resolved directly to that site's namespace:

- `/` resolves to `/index.html`;
- root-relative resources such as `/app.js` and `/style.css` resolve inside the same site;
- legacy `/site/<site-id>/...` paths remain supported;
- `/api`, `/api/...`, `/site`, and paths for another site are not exposed on that origin;
- only GET is allowed, and every path is percent-decoded and normalized before lookup.

The site-specific origin handler serves the site file before the node's normal UI/API route table, so a root-relative path cannot fall through to node control endpoints.

## Redirect query preservation

When a node-origin URL is redirected to its dedicated site origin, the original query string is retained. Empty queries are omitted and ASCII control characters are rejected before constructing the Location header.

## Regression coverage

The `awe-node` unit tests cover root path mapping, root-relative assets, cross-site/API namespace rejection, traversal rejection and quota arithmetic. The three-process product smoke test covers a real CSS request on the dedicated origin and a redirect carrying `lang` and `mode` query parameters.

These changes do not claim to implement AWEID session authorization, remote site replication, or independent physical-node recovery.
