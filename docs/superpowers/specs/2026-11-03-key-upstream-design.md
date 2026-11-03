# Per-key Agnes upstream selection

Approved in chat on 2026-11-03.

The account import dialog offers International (default), International alternate,
and China. Their fixed base URLs are `https://apihub.agnes-ai.com/v1`,
`https://apihub.agnes-ai.cn/v1`, and `https://api.agnes-ai.cn/v1`.
One selection applies to all new keys in a batch. Each record persists an
`upstream` identifier (`international`, `internationalBackup`, or `china`).
The account list displays this selection without exposing credentials.

No old-data migration or UI compatibility mode is required (user clarification).
New imports always persist an explicit selector; omitted/null API selectors default
to International. Duplicate imports preserve the existing record's upstream, including
when resetting failure state. No background credential migration or automatic
switching between services is introduced. Invalid selectors are rejected before
any writes. Malformed stored selectors cannot silently send a key to another service.

The dispatcher, individual key probe, model test, and upstream model listing all
resolve the base URL from the selected key. Existing task scheduling and failure handling remain in scope
only insofar as the selected key determines its request destination.

Validation covers import/serialization/reload, mixed-pool dispatch, default selection, duplicate preservation, malformed selectors, control-plane requests,
frontend translations, native tests, Worker build and local browser behavior.
