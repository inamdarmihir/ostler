"""Fixture tasks: what to type, and the exact submission the fixture server must have received.

Each step is a list of (label, kind, value). kind: text | check | select (value = visible option text).
`expect` is compared for EXACT equality with the single submission the server recorded.
"""

TASKS = [
    {
        "id": "basic", "name": "basic contact form (3 fields + checkbox)", "page": "/basic.html",
        "steps": [[("Full name", "text", "Ada Lovelace"), ("Email", "text", "ada@example.com"),
                   ("Message", "text", "Hello"), ("Subscribe to newsletter", "check", True)]],
        "submit": "Send message",
        "expect": {"name": "Ada Lovelace", "email": "ada@example.com", "message": "Hello", "newsletter": "yes"},
    },
    {
        "id": "validation", "name": "validation (3 rules, blur-validated)", "page": "/validation.html",
        "steps": [[("Email", "text", "ada@example.com"), ("Age", "text", "36"), ("ZIP code", "text", "94107")]],
        "submit": "Create account",
        "expect": {"email": "ada@example.com", "age": "36", "zip": "94107"},
    },
    {
        "id": "dependent", "name": "dependent dropdowns (async state list)", "page": "/dependent.html",
        "steps": [[("Recipient", "text", "Ada"), ("Country", "select", "Germany"),
                   ("State / region", "select", "Bavaria"), ("City", "text", "Munich")]],
        "submit": "Ship it",
        # Primitive-tool agents must fill in two batches: the state list is loaded by a fetch after Country changes.
        "split_after": 2, "wait_text": "Bavaria",
        "expect": {"recipient": "Ada", "country": "de", "state": "by", "city": "Munich"},
    },
    {
        "id": "wizard", "name": "3-step wizard", "page": "/wizard.html",
        "steps": [[("First name", "text", "Ada"), ("Last name", "text", "Lovelace")],
                  [("Email", "text", "ada@example.com"), ("Phone", "text", "5551234567")],
                  [("I confirm the details are correct", "check", True)]],
        "submit": ["Continue", "Continue", "Submit application"],
        "expect": {"first_name": "Ada", "last_name": "Lovelace", "email": "ada@example.com",
                   "phone": "5551234567", "terms": "on"},
    },
    {
        "id": "longform", "name": "long form (20 fields)", "page": "/longform.html",
        "steps": [[("First name", "text", "Ada"), ("Last name", "text", "Lovelace"),
                   ("Email", "text", "ada@example.com"), ("Phone", "text", "5551234567"),
                   ("Address line 1", "text", "12 Analytical St"), ("Address line 2", "text", "Suite 4"),
                   ("City", "text", "San Francisco"), ("State", "select", "California"),
                   ("ZIP code", "text", "94107"), ("Country", "select", "United States"),
                   ("Current company", "text", "Analytical Engines"), ("Job title", "text", "Engineer"),
                   ("LinkedIn URL", "text", "https://linkedin.com/in/ada"),
                   ("Website", "text", "https://ada.example.com"),
                   ("Earliest start date", "text", "2026-12-01"),
                   ("Expected salary (USD)", "text", "150000"),
                   ("How did you hear about us?", "select", "Web search"),
                   ("Cover letter", "text", "I would love to build the Engine."),
                   ("Willing to relocate", "check", True), ("I agree to the privacy policy", "check", True)]],
        "submit": "Submit application",
        "expect": {"first_name": "Ada", "last_name": "Lovelace", "email": "ada@example.com",
                   "phone": "5551234567", "address1": "12 Analytical St", "address2": "Suite 4",
                   "city": "San Francisco", "state": "ca", "zip": "94107", "country": "us",
                   "company": "Analytical Engines", "title": "Engineer",
                   "linkedin": "https://linkedin.com/in/ada", "website": "https://ada.example.com",
                   "start": "2026-12-01", "salary": "150000", "referral": "search",
                   "cover": "I would love to build the Engine.", "relocate": "on", "consent": "on"},
    },
]
