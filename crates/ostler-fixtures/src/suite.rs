//! The realistic-forms suite: 30 tasks, each a page plus the values an agent would send and the
//! ground-truth final state the *server* must have received.
//!
//! The pages are written from how common sites structure these forms (multi-step checkout, ATS
//! applications with EEO selects, government forms with split dates and masked ids, address and chip
//! widgets, shadow DOM, iframes, async checks). None is scraped from a real site.

use serde_json::{json, Value};

/// Placeholder in a task's values that the harness replaces with a real file path (the resume).
pub const RESUME: &str = "@resume";

pub enum Expect {
    /// The server must have received exactly these values (a subset match on the recorded submission).
    Submitted(Vec<(&'static str, Value)>),
    /// The page must NOT accept this: nothing may reach the server, and the tool output must say why
    /// (it must contain `must_mention`, case-insensitively).
    Refused { must_mention: &'static str },
}

pub struct Task {
    pub id: &'static str,
    /// The pattern this models, as seen on real sites.
    pub pattern: &'static str,
    pub page: &'static str,
    /// One entry per visible step: the values sent to `form_fill`, then `form_submit` is called.
    pub steps: Vec<Value>,
    pub expect: Expect,
}

fn t(
    id: &'static str,
    pattern: &'static str,
    page: &'static str,
    steps: Vec<Value>,
    expect: Expect,
) -> Task {
    Task {
        id,
        pattern,
        page,
        steps,
        expect,
    }
}

fn ok(v: Vec<(&'static str, Value)>) -> Expect {
    Expect::Submitted(v)
}

pub fn tasks() -> Vec<Task> {
    let ship = json!({"Full name":"Ada Lovelace","Street address":"12 Analytical St","City":"San Francisco","ZIP code":"94107","Country":"United States"});
    let ship_exp = |mut v: Vec<(&'static str, Value)>| {
        v.extend([
            ("ship_name", json!("Ada Lovelace")),
            ("ship_addr", json!("12 Analytical St")),
            ("ship_city", json!("San Francisco")),
            ("ship_zip", json!("94107")),
            ("ship_country", json!("us")),
            ("confirm", json!("yes")),
        ]);
        v
    };
    vec![
        t(
            "checkout_same_billing",
            "multi-step checkout, billing same as shipping",
            "/suite_checkout.html",
            vec![
                ship.clone(),
                json!({"Billing address is the same as shipping": true}),
                json!({"Delivery instructions":"Leave at the door","I confirm the order details":true}),
            ],
            ok(ship_exp(vec![
                ("bill_same", json!("yes")),
                ("bill_name", json!("Ada Lovelace")),
                ("bill_addr", json!("12 Analytical St")),
                ("bill_city", json!("San Francisco")),
                ("bill_zip", json!("94107")),
                ("note", json!("Leave at the door")),
            ])),
        ),
        t(
            "checkout_other_billing",
            "checkout with a different billing address revealed by the toggle",
            "/suite_checkout.html",
            vec![
                ship,
                json!({"Billing address is the same as shipping": false,"Billing name":"Ada Byron","Billing street address":"1 Byron Road","Billing city":"Oakland","Billing ZIP code":"94601"}),
                json!({"I confirm the order details":true}),
            ],
            ok(ship_exp(vec![
                ("bill_same", json!("no")),
                ("bill_name", json!("Ada Byron")),
                ("bill_addr", json!("1 Byron Road")),
                ("bill_city", json!("Oakland")),
                ("bill_zip", json!("94601")),
            ])),
        ),
        t(
            "job_application_eeo",
            "ATS application: resume upload, work-authorization radios, EEO selects",
            "/suite_job.html",
            vec![
                json!({"First name":"Ada","Last name":"Lovelace","Email":"ada@example.com","Phone":"5551234567","Resume/CV":RESUME,
                      "LinkedIn URL":"https://linkedin.com/in/ada","Cover letter":"I would love to build the Engine.",
                      "Are you legally authorized to work in the United States?":"Yes","Will you now or in the future require sponsorship?":"No",
                      "Gender":"Decline to self-identify","Race/Ethnicity":"Decline to self-identify",
                      "Veteran status":"I am not a protected veteran","Disability status":"I do not want to answer"}),
            ],
            ok(vec![
                ("first_name", json!("Ada")),
                ("email", json!("ada@example.com")),
                ("resume", json!("resume.pdf")),
                ("authorized", json!("yes")),
                ("sponsor", json!("no")),
                ("eeo_gender", json!("d")),
                ("eeo_race", json!("d")),
                ("eeo_veteran", json!("no")),
                ("eeo_disability", json!("d")),
                ("linkedin", json!("https://linkedin.com/in/ada")),
            ]),
        ),
        t(
            "tax_split_dob_masked_ids",
            "tax portal: DOB in three selects, PAN and Aadhaar masks, state to district",
            "/suite_tax_dob.html",
            vec![
                json!({"Name as per PAN":"Ada Lovelace","PAN":"abcde1234f","Aadhaar number":"123456789012","Day":"5","Month":"March","Year":"1990",
                      "State":"Karnataka","District":"Mysuru","Mobile number":"9876543210"}),
            ],
            ok(vec![
                ("pan", json!("ABCDE1234F")),
                ("aadhaar", json!("123456789012")),
                ("dob_day", json!("05")),
                ("dob_month", json!("03")),
                ("dob_year", json!("1990")),
                ("state", json!("KA")),
                ("district", json!("Mysuru")),
                ("mobile", json!("9876543210")),
            ]),
        ),
        t(
            "licence_dob_text_parts",
            "licence renewal: DOB as three short text boxes, pattern-checked id",
            "/suite_license_dob.html",
            vec![
                json!({"Licence number":"A1234567","Month":"3","Day":"5","Year":"1990","ZIP code":"94107","Last 4 digits of SSN":"1234"}),
            ],
            ok(vec![
                ("licence", json!("A1234567")),
                ("dob_m", json!("3")),
                ("dob_d", json!("5")),
                ("dob_y", json!("1990")),
                ("zip", json!("94107")),
                ("ssn4", json!("1234")),
            ]),
        ),
        t(
            "address_autocomplete",
            "street box with a suggestion list that fills city/state/ZIP",
            "/suite_address_auto.html",
            vec![
                json!({"Street address":"1600 Amphitheatre Parkway","City":"Mountain View","State":"CA","ZIP code":"94043"}),
            ],
            ok(vec![
                ("street", json!("1600 Amphitheatre Parkway")),
                ("city", json!("Mountain View")),
                ("state", json!("CA")),
                ("zip", json!("94043")),
            ]),
        ),
        t(
            "phone_country_code",
            "country-code dropdown plus a national number box that inserts a space",
            "/suite_phone_cc.html",
            vec![json!({"Country code":"India (+91)","Phone number":"9876543210"})],
            ok(vec![
                ("country_code", json!("+91")),
                ("national", json!("9876543210")),
            ]),
        ),
        t(
            "signup_password_rules",
            "signup: live password rules, confirm field, terms checkbox",
            "/suite_signup_pw.html",
            vec![
                json!({"Username":"ada_l","Email":"ada@example.com","Password":"Correct-Horse-9!","Confirm password":"Correct-Horse-9!","I agree to the Terms of Service":true}),
            ],
            ok(vec![
                ("username", json!("ada_l")),
                ("email", json!("ada@example.com")),
                ("password", json!("Correct-Horse-9!")),
                ("confirm", json!("Correct-Horse-9!")),
                ("tos", json!("1")),
            ]),
        ),
        t(
            "newsletter_consent_boxes",
            "separate consent boxes: only two of four may be ticked",
            "/suite_consent.html",
            vec![
                json!({"Email":"ada@example.com","Send me the weekly newsletter":true,"Tell me about product updates":true,
                      "Share my details with partners for their offers":false,"I have read the privacy policy":true}),
            ],
            ok(vec![
                ("email", json!("ada@example.com")),
                ("opt_news", json!("yes")),
                ("opt_product", json!("yes")),
                ("opt_partner", json!("no")),
                ("opt_sms", json!("no")),
                ("privacy", json!("yes")),
            ]),
        ),
        t(
            "radio_other_free_text",
            "radio group whose Other option reveals a text box",
            "/suite_radio_other.html",
            vec![
                json!({"How did you hear about us?":"Other","Other (please specify)":"Podcast","Anything else?":"Great product"}),
            ],
            ok(vec![
                ("source", json!("other")),
                ("source_other", json!("Podcast")),
                ("comment", json!("Great product")),
            ]),
        ),
        t(
            "flight_date_pickers",
            "two read-only inputs driven by a custom calendar popup",
            "/suite_flight_dates.html",
            vec![json!({"Departure date":"2026-12-05","Return date":"2026-12-12"})],
            ok(vec![
                ("depart", json!("2026-12-05")),
                ("return", json!("2026-12-12")),
            ]),
        ),
        t(
            "contenteditable_body",
            "mail composer with a contenteditable body",
            "/suite_richtext.html",
            vec![
                json!({"To":"team@example.com","Subject":"Weekly notes","Message body":"Shipped the beta."}),
            ],
            ok(vec![
                ("to", json!("team@example.com")),
                ("subject", json!("Weekly notes")),
                ("body", json!("Shipped the beta.")),
            ]),
        ),
        t(
            "shadow_dom_nested",
            "inputs two shadow roots deep",
            "/suite_shadow_nested.html",
            vec![json!({"Company name":"Analytical Engines","VAT number":"GB123456789"})],
            ok(vec![
                ("company", json!("Analytical Engines")),
                ("vat", json!("GB123456789")),
            ]),
        ),
        t(
            "iframe_hosted_form",
            "same-origin iframe loaded by src",
            "/suite_iframe_host.html",
            vec![
                json!({"Company":"Analytical Engines","Team size":"11-50","Work email":"ada@analytical.example"}),
            ],
            ok(vec![
                ("company", json!("Analytical Engines")),
                ("size", json!("11-50")),
                ("work_email", json!("ada@analytical.example")),
            ]),
        ),
        t(
            "reveal_employed",
            "a select reveals Employer; income box formats as currency",
            "/suite_reveal.html",
            vec![
                json!({"Employment status":"Employed","Employer name":"Analytical Engines","Annual income (USD)":"150000"}),
            ],
            ok(vec![
                ("status", json!("employed")),
                ("employer", json!("Analytical Engines")),
                ("business", json!("")),
                ("ein", json!("")),
                ("income", json!("150000")),
            ]),
        ),
        t(
            "reveal_self_employed",
            "same select, other branch reveals business name and tax id",
            "/suite_reveal.html",
            vec![
                json!({"Employment status":"Self-employed","Business name":"Lovelace Consulting","Tax ID (EIN)":"12-3456789","Annual income (USD)":"90000"}),
            ],
            ok(vec![
                ("status", json!("self")),
                ("employer", json!("")),
                ("business", json!("Lovelace Consulting")),
                ("ein", json!("12-3456789")),
                ("income", json!("90000")),
            ]),
        ),
        t(
            "async_username_free",
            "inline async username check that enables the button",
            "/suite_async_user.html",
            vec![json!({"Username":"ada_lovelace","Email":"ada@example.com"})],
            ok(vec![
                ("username", json!("ada_lovelace")),
                ("email", json!("ada@example.com")),
            ]),
        ),
        t(
            "csrf_and_hidden_fields",
            "native post with CSRF token, hidden ids and a honeypot",
            "/suite_csrf.html",
            vec![json!({"Display name":"Ada","Bio":"Mathematician"})],
            ok(vec![
                ("csrf_token", json!("tok_7f3a9c21")),
                ("form_id", json!("profile_v2")),
                ("return_to", json!("/account")),
                ("display_name", json!("Ada")),
                ("bio", json!("Mathematician")),
                ("website_hp", json!("")),
            ]),
        ),
        t(
            "toggle_switches",
            "role=switch buttons, one already on",
            "/suite_switches.html",
            vec![
                json!({"Email alerts":true,"Push notifications":false,"Weekly digest":true,"Digest frequency":"Weekly"}),
            ],
            ok(vec![
                ("email", json!("true")),
                ("push", json!("false")),
                ("digest", json!("true")),
                ("freq", json!("weekly")),
            ]),
        ),
        t(
            "datalist_and_optgroups",
            "input with a datalist and a select with optgroups",
            "/suite_datalist.html",
            vec![json!({"Country":"Germany","Timezone":"Berlin (UTC+1)"})],
            ok(vec![
                ("country", json!("Germany")),
                ("tz", json!("europe/berlin")),
            ]),
        ),
        t(
            "chip_input",
            "multi-value chip input fed by a suggestion list",
            "/suite_chips.html",
            vec![json!({"Skills":["Rust","Go"]})],
            ok(vec![("skills", json!(["Go", "Rust"]))]),
        ),
        t(
            "custom_listbox_select",
            "button + listbox dropdown (Headless UI style), number box",
            "/suite_custom_select.html",
            vec![json!({"Billing cycle":"Yearly (save 20%)","Seats":3})],
            ok(vec![("cycle", json!("yearly")), ("seats", json!("3"))]),
        ),
        t(
            "radio_cards",
            "visually hidden radios inside clickable cards",
            "/suite_radio_cards.html",
            vec![json!({"Shipping speed":"Express","Gift message":"Happy birthday"})],
            ok(vec![
                ("speed", json!("express")),
                ("gift", json!("Happy birthday")),
            ]),
        ),
        // Everything below SHOULD NOT reach the server, and the tool must say why.
        t(
            "fail_username_taken",
            "async check says the username is taken",
            "/suite_async_user.html",
            vec![json!({"Username":"admin","Email":"ada@example.com"})],
            Expect::Refused {
                must_mention: "taken",
            },
        ),
        t(
            "fail_captcha_gate",
            "CAPTCHA widget in the form",
            "/suite_captcha_gate.html",
            vec![json!({"Name":"Ada","Message":"Hello"})],
            Expect::Refused {
                must_mention: "captcha",
            },
        ),
        t(
            "fail_wrong_format",
            "value that does not match the required pattern",
            "/suite_wrong_format.html",
            vec![json!({"Plate number (format AB-1234)":"1234","Owner":"Ada"})],
            Expect::Refused {
                must_mention: "AB-1234",
            },
        ),
        t(
            "fail_server_rejection",
            "server answers 422 with a message",
            "/suite_server_reject.html",
            vec![json!({"Voucher code":"OLD2020","Email":"ada@example.com"})],
            Expect::Refused {
                must_mention: "expired",
            },
        ),
        t(
            "fail_disabled_field",
            "asked to fill a disabled control",
            "/suite_disabled_field.html",
            vec![json!({"Account holder":"Ada","Account number":"123456789","Nickname":"Main"})],
            Expect::Refused {
                must_mention: "disabled",
            },
        ),
        t(
            "fail_terms_need_scroll",
            "terms checkbox stays disabled until the terms are scrolled",
            "/suite_terms_scroll.html",
            vec![json!({"I have read and accept the terms":true,"Full name":"Ada"})],
            Expect::Refused {
                must_mention: "disabled",
            },
        ),
        t(
            "fail_field_not_on_page",
            "asked for a field the page does not have",
            "/suite_missing_field.html",
            vec![
                json!({"Email":"ada@example.com","Date of birth":"1990-03-05","Mother's maiden name":"Smith"}),
            ],
            Expect::Refused {
                must_mention: "maiden",
            },
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_suite_has_30_tasks_with_unique_ids_and_served_pages() {
        let ts = tasks();
        assert_eq!(ts.len(), 30);
        let mut ids: Vec<_> = ts.iter().map(|t| t.id).collect();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), 30);
        for t in &ts {
            let name = t.page.trim_start_matches('/').trim_end_matches(".html");
            assert!(crate::PAGES.iter().any(|(n, _)| *n == name), "{}", t.page);
        }
        assert!(
            ts.iter()
                .filter(|t| matches!(t.expect, Expect::Refused { .. }))
                .count()
                >= 5
        );
    }
}
