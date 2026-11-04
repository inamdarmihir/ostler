# Safety

Browser-level enforcement and its known limits. Back to the [README](../README.md).

## Safety

- `--allow-domain` is enforced **inside the browser**. Every request the page makes is checked, whatever its type (documents, images, stylesheets, scripts, `fetch`/XHR, beacons, event streams), and Chrome is launched so that every other host does not resolve, which also covers WebSockets and the first request of a popup. A request to another host never leaves. An entry without a port allows every port; `host:8443` pins one.
- **File fields are refused** unless you pass `--upload-dir`; paths are resolved first, so `..` and symlinks cannot escape the directory. A page's `confirm()` and `prompt()` dialogs are **dismissed**, not clicked through, unless you pass `--accept-dialogs`.
- **No debugging port.** Chrome talks to Ostler over a pipe, so no other local user can drive the browser (`OSTLER_CDP_TRANSPORT=ws` brings back the old localhost port for debugging).
- `--require-confirm-submit` blocks **every non-GET request** (click, Enter, page script) unless it happens inside a confirmed `form_submit`. A GET with side effects is not covered.
- Page text is wrapped as untrusted (the wrapper cannot be closed from inside), including titles, URLs and the `look` legend. Text a person cannot see (opacity 0, zero font size, far off-screen, same colour as the background) is left out of snapshots and counted, so a hidden instruction never reaches the model. Secrets are refused by memory and masked in reports, snapshots and notes; memory never overwrites a fact you told it.
- **CAPTCHAs and verification walls are reported, never solved or bypassed.** reCAPTCHA, hCaptcha and Turnstile widgets and "verify you are human" interstitials put a `CHALLENGE:` line at the top of the tool result, so the agent stops retrying and asks the user. With `--headed`, `wait_for_human` waits while the person completes it in the window. A2A tasks come back `input-required`. Invisible badges and ordinary pages that mention verification are not flagged.
- Popups open as tabs under the same policy. A crashed Chrome is restarted and the agent is told.

**Still open:** an empty allowlist permits loopback and private addresses unless you pass `--block-private-networks` (which a DNS-rebinding server can still outwit, so prefer `--allow-domain`), a port pin is not isolation for WebSockets, and nothing here has been audited by a third party. For anything sensitive use `--allow-domain`, an `--upload-dir` with nothing secret in it, and a throwaway profile. Status of every finding: [review](REVIEW-2026-10.md#status). Policy and threat model: [SECURITY.md](../SECURITY.md).
