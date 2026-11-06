# Pack: gov-forms

A synthetic county permit office ("Riverton County Permit Office") with a 3-step building-permit wizard. Served at `/packs/gov-forms/...`; tasks in `tasks.json`.

**These are synthetic replicas of widget shapes, not real sites.** The office, the parcel format (`PX-12345`) and the confirmation numbers are invented. It is not modelled on any real agency's portal.

## Covers

- A 3-step wizard (applicant, property, review and certify) on one page with a real multipart `POST` to `/packs/gov-forms/submit`.
- Address fields (street, city, postal code) with autocomplete attributes.
- A file upload (`site_plan`); the record stores the file name.
- Conditional sections: the business registration field (applicant type = Business) and the elevation certificate field (flood zone = yes). Hidden conditional fields are disabled and so not submitted.
- A review step that echoes what will be sent, and a certification checkbox.
- A receipt page with a confirmation number (`GF-nnnnn`, a deterministic hash of applicant name and parcel, so a made-up number is detectable).
- A server-side validation rejection: the client accepts any parcel ID, the server answers `422` with an error list when the parcel is not `PX-` plus five digits (task 005).

## Does NOT cover

- Real agency widgets: address autocomplete services, identity proofing, payment of fees, e-signature, CAPTCHA, session timeouts, multi-page server-rendered wizards.
- Anything about legal correctness of permit rules.

## Harness notes

Tasks tagged `needs-file:<name>` expect a file with that name in the agent's upload directory (the bench creates a tiny placeholder PDF). Records are the ground truth: the agent's claim of a confirmation number is only accepted when it equals the number the server issues.
