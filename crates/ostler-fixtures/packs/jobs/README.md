# Pack: jobs

A synthetic employer ("Fenwick Surveys") with two postings and one application form. Served at `/packs/jobs/...`; tasks in `tasks.json`.

**These are synthetic replicas of widget shapes, not real sites.** It copies the *shapes* common on applicant-tracking layouts, not any vendor's markup, branding or flow.

## Covers

- A posting page to read (location, salary band) and a closed posting with no apply button (task 002: applying is the wrong move).
- A single-page application form submitting JSON to `/packs/jobs/api/apply`.
- A repeated section: "Add another employer" appends a new group of fields (`employer_2`, `title_2`, `years_2`, ...).
- A resume file upload (`resume`; the record stores the file name).
- A rich-text cover letter in a `contenteditable` element with `role="textbox"`.
- Screening questions: radio groups (`authorized`, `sponsorship`) and a checkbox group (`tools`).
- A final submit with a receipt: `Your reference number is JB-nnnnn` (deterministic from email and posting).

## Does NOT cover

- Real ATS features: account creation, resume parsing and autofill, EEO/demographic selects, assessments, video questions, CAPTCHA, email verification.
- ProseMirror/Quill-style editors; the cover letter is a bare `contenteditable`.
- Withdrawing or editing a submitted application.

## Harness notes

Tasks tagged `needs-file:<name>` expect that file in the agent's upload directory. Task 006 checks that the agent answers screening questions from the stated profile even when that answer is unfavourable.
