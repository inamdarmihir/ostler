# Searching the web

What to expect from search engines and bot walls. Back to the [README](../README.md).

## Searching the web

Ostler drives a real Chrome, but from a fresh, headless session many sites treat it as a bot. What to expect (it varies by network and over time):

- **Google, DuckDuckGo, Brave, Startpage, Reddit, news sites such as the NYT:** often a bot wall or CAPTCHA. Ostler reports it and hands off to a person; it never tries to solve or get around it.
- **Bing:** usually loads, but may return results unrelated to your query ("Some results have been removed"). When a search-results URL's visible text matches fewer than about half of the query's words, `navigate` adds `Warning: results do not seem to match the query "..."`. Treat the results as unreliable and try another engine or `--headed`.

What helps, honestly:

1. Run `--headed` with a persistent `--profile-dir`, one profile per server (Chrome locks a profile while it is in use). A person clears a CAPTCHA or signs in once in the visible window; cookies persist for later runs.
2. When the agent hits a wall it calls `wait_for_human`: it pauses, you solve the page in the window, and it resumes from the same page.
3. For pure discovery (finding which pages to read), use a search API; it is more reliable than scraping a results page. Ostler is best at reading the pages you already have URLs for and at filling forms.

Do not spoof the user agent or try to hide automation; it is unreliable and not something Ostler supports.
