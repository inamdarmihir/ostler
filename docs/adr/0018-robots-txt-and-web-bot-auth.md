# ADR-0018: robots.txt per RFC 9309 now; Web Bot Auth later, once it is more than a working-group draft

Status: proposed (M0, document-based, 2026-11-06). Answers the "Crawl etiquette" and "Agent identity, later" rows of BUILD-PLAN section 4; relates to [ADR-0009](0009-challenges-are-handed-off.md).

## Context
M3 adds `open_many` and crawling; the plan defaults to respecting robots.txt "per RFC 9309" with an override, and keeps Web Bot Auth as a possible later way for the agent to identify itself. This ADR records what the two documents say and what that means for Ostler. It is a document check; no code was written or run.

## Decision
**robots.txt (adopt, M3).**
1. Implement RFC 9309 behaviour for requests Ostler makes on its own (`open_many`, crawling, fetching sources). A navigation the **user's agent asks for explicitly** in the browser is not a crawl; the default there is to proceed and say nothing. The override flag applies to the automatic paths only. State this split in docs, because robots.txt is "not a form of access authorization" (RFC 9309, sections 1 and 3): it is etiquette, not a security boundary, and Ostler must not present it as one.
2. Behaviours to implement and test (RFC 9309 section numbers):
   - 4xx for robots.txt means "unavailable": crawling is allowed (2.3.1.3); 5xx or an unreachable server means assume complete disallow (2.3.1.4);
   - follow at least five consecutive redirects; more than five may be treated as unavailable (2.3.1.2);
   - cache for at most 24 hours unless the file is unreachable (2.4);
   - parse at least 500 KiB (2.5);
   - the most specific rule wins, measured by octets; an equal Allow and Disallow resolve to Allow (2.2.2);
   - user-agent groups match case-insensitively, matching groups are combined, `*` applies if none match (2.2.1).
3. Use a product token (for example `Ostler`) in the User-Agent so sites can address us, and report the decision (`robots: disallowed by /private`) in the tool result instead of failing silently ([ADR-0011](0011-reliability-principles.md), principle 1).
4. **Parser choice is open.** Candidates exist (below) but are old; either vendor-check one against RFC test vectors in M3 or write the ~200-line parser with fuzzing. Do not adopt one on download counts alone.

**Web Bot Auth (defer).** Do not implement now. Revisit when the IETF work reaches last call or a site we care about requires it. The design must not block it: requests go through one outbound HTTP path, so adding signing later is one change. ADR-0009 stands: Ostler does not evade bot protection; identifying ourselves honestly is the compatible direction, and signing is how.

## Rejected alternatives
- *Ignore robots.txt because the agent acts for a user.* The plan's default respects it for automatic fetching; a person-initiated page view differs from automatic crawling, which is why Decision 1 splits them.
- *Implement Web Bot Auth now.* The specification is a revision-01 working-group draft; signing keys, key directory hosting and which sites verify are all undecided in practice.
- *Spoof a browser UA to avoid blocks.* Contradicts ADR-0009 and the plan's "no evasion" stance.

## Consequences
- Research mode needs a per-host robots cache with a 24 h TTL and a bounded size.
- Some sites will refuse automatic fetching of pages a human can open. That is expected; the result says why and how to override.
- Web Bot Auth work, when it comes, needs a key store (the sealing key material is the obvious neighbour) and a place to publish a key directory, which a local tool does not have. That is a real obstacle, not a detail.

## Verification
Documents read 2026-11-06 through a page-query tool (the sandbox blocks rfc-editor.org and datatracker.ietf.org for direct fetches); answers are the tool's extraction from the cited pages.

| claim | source | result |
|---|---|---|
| RFC 9309 status | rfc-editor.org/info/rfc9309 and rfc-editor.org/rfc/rfc9309.html | "Robots Exclusion Protocol", **Proposed Standard**, September 2022, authors Koster, Illyes, Zeller, Sassman |
| 4xx vs 5xx, redirects, cache, size, precedence, groups, not authorization | the RFC text, sections as cited above | as listed in Decision 2; the 24 h and 500 KiB figures are also in `draft-koster-rep-09` (search result excerpt) |
| Web Bot Auth status | datatracker.ietf.org/wg/webbotauth/ and `draft-ietf-webbotauth-httpsig-protocol` | working group exists; protocol draft **`-01`, 29 October 2026**, "Active Internet-Draft", intended RFC status not set, expires 2 May 2027 (the group page listed `-00` of 2026-09-01 when it was cached). It defines a `Signature-Agent` header for in-band key discovery, a JWKS-based key directory at a well-known URI, and a mandatory `tag="web-bot-auth"` signature parameter, on top of HTTP Message Signatures (RFC 9421). An earlier architecture draft (`draft-meunier-web-bot-auth-architecture-05`, 2026-03-02) is marked replaced |
| Rust robots parsers (metadata only) | crates.io API | `texting_robots` 0.2.2 (2023-03-29, MIT OR Apache-2.0); `robotstxt` 0.3.0 (2021-02-13, Apache-2.0, a port of Google's parser); `robotparser-rs` 0.1.0 (2026-06-23, MIT, ~9 recent downloads) |

**Not verified (UNVERIFIED):** that any listed crate implements the RFC behaviours above (none was run); which sites verify Web Bot Auth signatures today; whether the draft will change before last call (drafts expire and are replaced); legal treatment of robots.txt in any jurisdiction (outside this ADR).
