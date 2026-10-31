# ADR-0009: Verification walls are detected and handed to a person

Status: accepted (2026-10-31)

## Context
Agents that hit a CAPTCHA tend to spend many turns clicking at it, because to them it looks like a generic failure. Products in this space answer with solver services, stealth fingerprints and proxies. Those work by defeating a site's deliberate decision to keep automated clients out, which carries terms-of-service and legal risk, starts an arms race that is paid for forever, and conflicts with how Ostler already describes itself ("does not try to evade it").

## Decision
Ostler **detects** verification walls and **hands them to a person**. It never solves, bypasses or disguises itself.

- **Detection** (`runtime.js: challenge()`): embedded widgets (iframes to reCAPTCHA, hCaptcha, Turnstile, Arkose, DataDome, or their standard container classes) that are visible and at least 200x50 px, and interstitials that replace the page (a "Just a moment" style title, or short pages saying "verify you are human" that have no input fields). The invisible reCAPTCHA v3 badge, hidden widgets and ordinary pages that merely mention verification are not flagged. A negative fixture caught a false positive on first run (a short page with a form that mentioned verification), which is why the body-text rule requires no inputs.
- **Reporting**: after any page-affecting tool, a `CHALLENGE:` line is put first in the result, naming the vendor and telling the agent not to retry or click around it.
- **Handoff**: `wait_for_human` polls until the wall clears, so the person can complete it in the window. It requires `--headed`; in a headless browser it says so immediately instead of waiting for someone who cannot see the page.
- **Delegation**: an A2A task that lands on a wall returns `input-required` with the vendor, and nothing is typed or submitted behind the wall.

## Rejected
- Solver services, vision-model solving and fingerprint spoofing (see Context).
- Auto-relaunching in headed mode on detection. It would silently move a user's automation onto their screen. A flag the user chose is more honest.

## Consequences
- Tasks behind a wall cannot finish unattended. That's the point, and the agent now knows within one call rather than after many.
- Detection is heuristic. Walls Ostler does not recognise (custom or novel ones) still look like generic failures. Fixtures cover the three common shapes only, with stand-ins rather than real third-party widgets.
- Later options that stay on the right side of the line: a persistent profile and saved auth state (fewer challenges for real users), remembering in Qdrant which sites tend to challenge, and signed-agent standards such as Web Bot Auth if sites adopt them.
