//! spall's rules (#41): the `ruleId`s its candidates carry. Each is an
//! OWASP WSTG v4.2 test id (how it was tested), mapped to a CWE (what it
//! is), built as `flux::Rule` values. Pinned to v4.2: ids are stable per
//! tool (flux SPEC.md), so moving to a newer WSTG is a deliberate
//! migration. Ids and titles are OWASP's (WSTG, CC BY-SA 4.0), checked
//! against the v4.2 tag of github.com/OWASP/wstg; the CWE mapping is ours.
//!
//! Curated: the tests that can end in a finding. `spall/other` is the
//! escape hatch, so a finding never waits on the catalog.

use serde::Serialize;

pub const WSTG_VERSION: &str = "4.2";
pub const ATTRIBUTION: &str = "Rule ids and titles: OWASP Web Security Testing Guide v4.2 (CC BY-SA 4.0). CWE mapping: spall.";
pub const OTHER: &str = "spall/other";
const HELP_BASE: &str = "https://owasp.org/www-project-web-security-testing-guide/v42/4-Web_Application_Security_Testing/";

/// One catalog entry, as the UI and console list it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Entry {
    pub id: &'static str,
    pub title: &'static str,
    /// `None` only for `spall/other`.
    pub cwe: Option<u32>,
    #[serde(skip)]
    path: &'static str,
}

impl Entry {
    pub fn help_uri(&self) -> Option<String> {
        (!self.path.is_empty()).then(|| format!("{HELP_BASE}{}", self.path))
    }

    /// As a flux rule (SARIF `reportingDescriptor`), with its CWE.
    pub fn to_flux(&self) -> flux::Rule {
        let mut r = flux::Rule::new(self.id, self.title);
        r.help_uri = self.help_uri();
        match self.cwe {
            Some(c) => r.with_cwe(c),
            None => r,
        }
    }
}

const fn e(id: &'static str, title: &'static str, cwe: u32, path: &'static str) -> Entry {
    Entry {
        id,
        title,
        cwe: Some(cwe),
        path,
    }
}

const OTHER_ENTRY: Entry = Entry {
    id: OTHER,
    title: "Other (not in the catalog)",
    cwe: None,
    path: "",
};

/// The catalog, in WSTG order, then `spall/other`.
pub static CATALOG: &[Entry] = &[
    e("WSTG-INFO-02", "Fingerprint Web Server", 200, "01-Information_Gathering/02-Fingerprint_Web_Server"),
    e("WSTG-INFO-05", "Review Webpage Content for Information Leakage", 200, "01-Information_Gathering/05-Review_Webpage_Content_for_Information_Leakage"),
    e("WSTG-CONF-04", "Review Old Backup and Unreferenced Files for Sensitive Information", 530, "02-Configuration_and_Deployment_Management_Testing/04-Review_Old_Backup_and_Unreferenced_Files_for_Sensitive_Information"),
    e("WSTG-CONF-05", "Enumerate Infrastructure and Application Admin Interfaces", 425, "02-Configuration_and_Deployment_Management_Testing/05-Enumerate_Infrastructure_and_Application_Admin_Interfaces"),
    e("WSTG-CONF-06", "Test HTTP Methods", 749, "02-Configuration_and_Deployment_Management_Testing/06-Test_HTTP_Methods"),
    e("WSTG-CONF-07", "Test HTTP Strict Transport Security", 319, "02-Configuration_and_Deployment_Management_Testing/07-Test_HTTP_Strict_Transport_Security"),
    e("WSTG-CONF-10", "Test for Subdomain Takeover", 350, "02-Configuration_and_Deployment_Management_Testing/10-Test_for_Subdomain_Takeover"),
    e("WSTG-CONF-11", "Test Cloud Storage", 284, "02-Configuration_and_Deployment_Management_Testing/11-Test_Cloud_Storage"),
    e("WSTG-ATHN-01", "Testing for Credentials Transported over an Encrypted Channel", 319, "04-Authentication_Testing/01-Testing_for_Credentials_Transported_over_an_Encrypted_Channel"),
    e("WSTG-ATHN-02", "Testing for Default Credentials", 1392, "04-Authentication_Testing/02-Testing_for_Default_Credentials"),
    e("WSTG-ATHN-03", "Testing for Weak Lock Out Mechanism", 307, "04-Authentication_Testing/03-Testing_for_Weak_Lock_Out_Mechanism"),
    e("WSTG-ATHN-04", "Testing for Bypassing Authentication Schema", 287, "04-Authentication_Testing/04-Testing_for_Bypassing_Authentication_Schema"),
    e("WSTG-ATHN-05", "Testing for Vulnerable Remember Password", 312, "04-Authentication_Testing/05-Testing_for_Vulnerable_Remember_Password"),
    e("WSTG-ATHN-06", "Testing for Browser Cache Weaknesses", 525, "04-Authentication_Testing/06-Testing_for_Browser_Cache_Weaknesses"),
    e("WSTG-ATHN-07", "Testing for Weak Password Policy", 521, "04-Authentication_Testing/07-Testing_for_Weak_Password_Policy"),
    e("WSTG-ATHN-08", "Testing for Weak Security Question Answer", 640, "04-Authentication_Testing/08-Testing_for_Weak_Security_Question_Answer"),
    e("WSTG-ATHN-09", "Testing for Weak Password Change or Reset Functionalities", 640, "04-Authentication_Testing/09-Testing_for_Weak_Password_Change_or_Reset_Functionalities"),
    e("WSTG-ATHN-10", "Testing for Weaker Authentication in Alternative Channel", 287, "04-Authentication_Testing/10-Testing_for_Weaker_Authentication_in_Alternative_Channel"),
    e("WSTG-ATHZ-01", "Testing Directory Traversal File Include", 22, "05-Authorization_Testing/01-Testing_Directory_Traversal_File_Include"),
    e("WSTG-ATHZ-02", "Testing for Bypassing Authorization Schema", 285, "05-Authorization_Testing/02-Testing_for_Bypassing_Authorization_Schema"),
    e("WSTG-ATHZ-03", "Testing for Privilege Escalation", 269, "05-Authorization_Testing/03-Testing_for_Privilege_Escalation"),
    e("WSTG-ATHZ-04", "Testing for Insecure Direct Object References", 639, "05-Authorization_Testing/04-Testing_for_Insecure_Direct_Object_References"),
    e("WSTG-SESS-01", "Testing for Session Management Schema", 330, "06-Session_Management_Testing/01-Testing_for_Session_Management_Schema"),
    e("WSTG-SESS-02", "Testing for Cookies Attributes", 614, "06-Session_Management_Testing/02-Testing_for_Cookies_Attributes"),
    e("WSTG-SESS-03", "Testing for Session Fixation", 384, "06-Session_Management_Testing/03-Testing_for_Session_Fixation"),
    e("WSTG-SESS-04", "Testing for Exposed Session Variables", 598, "06-Session_Management_Testing/04-Testing_for_Exposed_Session_Variables"),
    e("WSTG-SESS-05", "Testing for Cross Site Request Forgery", 352, "06-Session_Management_Testing/05-Testing_for_Cross_Site_Request_Forgery"),
    e("WSTG-SESS-06", "Testing for Logout Functionality", 613, "06-Session_Management_Testing/06-Testing_for_Logout_Functionality"),
    e("WSTG-SESS-07", "Testing Session Timeout", 613, "06-Session_Management_Testing/07-Testing_Session_Timeout"),
    e("WSTG-SESS-08", "Testing for Session Puzzling", 488, "06-Session_Management_Testing/08-Testing_for_Session_Puzzling"),
    e("WSTG-SESS-09", "Testing for Session Hijacking", 294, "06-Session_Management_Testing/09-Testing_for_Session_Hijacking"),
    e("WSTG-INPV-01", "Testing for Reflected Cross Site Scripting", 79, "07-Input_Validation_Testing/01-Testing_for_Reflected_Cross_Site_Scripting"),
    e("WSTG-INPV-02", "Testing for Stored Cross Site Scripting", 79, "07-Input_Validation_Testing/02-Testing_for_Stored_Cross_Site_Scripting"),
    e("WSTG-INPV-03", "Testing for HTTP Verb Tampering", 650, "07-Input_Validation_Testing/03-Testing_for_HTTP_Verb_Tampering"),
    e("WSTG-INPV-04", "Testing for HTTP Parameter Pollution", 235, "07-Input_Validation_Testing/04-Testing_for_HTTP_Parameter_Pollution"),
    e("WSTG-INPV-05", "Testing for SQL Injection", 89, "07-Input_Validation_Testing/05-Testing_for_SQL_Injection"),
    e("WSTG-INPV-06", "Testing for LDAP Injection", 90, "07-Input_Validation_Testing/06-Testing_for_LDAP_Injection"),
    e("WSTG-INPV-07", "Testing for XML Injection", 611, "07-Input_Validation_Testing/07-Testing_for_XML_Injection"),
    e("WSTG-INPV-08", "Testing for SSI Injection", 97, "07-Input_Validation_Testing/08-Testing_for_SSI_Injection"),
    e("WSTG-INPV-09", "Testing for XPath Injection", 643, "07-Input_Validation_Testing/09-Testing_for_XPath_Injection"),
    e("WSTG-INPV-10", "Testing for IMAP SMTP Injection", 93, "07-Input_Validation_Testing/10-Testing_for_IMAP_SMTP_Injection"),
    e("WSTG-INPV-11", "Testing for Code Injection", 94, "07-Input_Validation_Testing/11-Testing_for_Code_Injection"),
    e("WSTG-INPV-12", "Testing for Command Injection", 78, "07-Input_Validation_Testing/12-Testing_for_Command_Injection"),
    e("WSTG-INPV-15", "Testing for HTTP Splitting Smuggling", 444, "07-Input_Validation_Testing/15-Testing_for_HTTP_Splitting_Smuggling"),
    e("WSTG-INPV-17", "Testing for Host Header Injection", 20, "07-Input_Validation_Testing/17-Testing_for_Host_Header_Injection"),
    e("WSTG-INPV-18", "Testing for Server-side Template Injection", 1336, "07-Input_Validation_Testing/18-Testing_for_Server-side_Template_Injection"),
    e("WSTG-INPV-19", "Testing for Server-Side Request Forgery", 918, "07-Input_Validation_Testing/19-Testing_for_Server-Side_Request_Forgery"),
    e("WSTG-ERRH-01", "Testing for Improper Error Handling", 209, "08-Testing_for_Error_Handling/01-Testing_For_Improper_Error_Handling"),
    e("WSTG-ERRH-02", "Testing for Stack Traces", 209, "08-Testing_for_Error_Handling/02-Testing_for_Stack_Traces"),
    e("WSTG-CRYP-01", "Testing for Weak Transport Layer Security", 326, "09-Testing_for_Weak_Cryptography/01-Testing_for_Weak_Transport_Layer_Security"),
    e("WSTG-CRYP-02", "Testing for Padding Oracle", 209, "09-Testing_for_Weak_Cryptography/02-Testing_for_Padding_Oracle"),
    e("WSTG-CRYP-03", "Testing for Sensitive Information Sent via Unencrypted Channels", 319, "09-Testing_for_Weak_Cryptography/03-Testing_for_Sensitive_Information_Sent_via_Unencrypted_Channels"),
    e("WSTG-CRYP-04", "Testing for Weak Encryption", 327, "09-Testing_for_Weak_Cryptography/04-Testing_for_Weak_Encryption"),
    e("WSTG-BUSL-01", "Test Business Logic Data Validation", 20, "10-Business_Logic_Testing/01-Test_Business_Logic_Data_Validation"),
    e("WSTG-BUSL-02", "Test Ability to Forge Requests", 807, "10-Business_Logic_Testing/02-Test_Ability_to_Forge_Requests"),
    e("WSTG-BUSL-03", "Test Integrity Checks", 354, "10-Business_Logic_Testing/03-Test_Integrity_Checks"),
    e("WSTG-BUSL-04", "Test for Process Timing", 208, "10-Business_Logic_Testing/04-Test_for_Process_Timing"),
    e("WSTG-BUSL-05", "Test Number of Times a Function Can Be Used Limits", 799, "10-Business_Logic_Testing/05-Test_Number_of_Times_a_Function_Can_Be_Used_Limits"),
    e("WSTG-BUSL-06", "Testing for the Circumvention of Work Flows", 841, "10-Business_Logic_Testing/06-Testing_for_the_Circumvention_of_Work_Flows"),
    e("WSTG-BUSL-07", "Test Defenses Against Application Misuse", 693, "10-Business_Logic_Testing/07-Test_Defenses_Against_Application_Misuse"),
    e("WSTG-BUSL-08", "Test Upload of Unexpected File Types", 434, "10-Business_Logic_Testing/08-Test_Upload_of_Unexpected_File_Types"),
    e("WSTG-BUSL-09", "Test Upload of Malicious Files", 434, "10-Business_Logic_Testing/09-Test_Upload_of_Malicious_Files"),
    e("WSTG-CLNT-01", "Testing for DOM-Based Cross Site Scripting", 79, "11-Client-side_Testing/01-Testing_for_DOM-based_Cross_Site_Scripting"),
    e("WSTG-CLNT-02", "Testing for JavaScript Execution", 79, "11-Client-side_Testing/02-Testing_for_JavaScript_Execution"),
    e("WSTG-CLNT-03", "Testing for HTML Injection", 80, "11-Client-side_Testing/03-Testing_for_HTML_Injection"),
    e("WSTG-CLNT-04", "Testing for Client-side URL Redirect", 601, "11-Client-side_Testing/04-Testing_for_Client-side_URL_Redirect"),
    e("WSTG-CLNT-05", "Testing for CSS Injection", 79, "11-Client-side_Testing/05-Testing_for_CSS_Injection"),
    e("WSTG-CLNT-06", "Testing for Client-side Resource Manipulation", 829, "11-Client-side_Testing/06-Testing_for_Client-side_Resource_Manipulation"),
    e("WSTG-CLNT-07", "Testing Cross Origin Resource Sharing", 942, "11-Client-side_Testing/07-Testing_Cross_Origin_Resource_Sharing"),
    e("WSTG-CLNT-09", "Testing for Clickjacking", 1021, "11-Client-side_Testing/09-Testing_for_Clickjacking"),
    e("WSTG-CLNT-10", "Testing WebSockets", 1385, "11-Client-side_Testing/10-Testing_WebSockets"),
    e("WSTG-CLNT-11", "Testing Web Messaging", 345, "11-Client-side_Testing/11-Testing_Web_Messaging"),
    e("WSTG-CLNT-12", "Testing Browser Storage", 922, "11-Client-side_Testing/12-Testing_Browser_Storage"),
    e("WSTG-CLNT-13", "Testing for Cross Site Script Inclusion", 200, "11-Client-side_Testing/13-Testing_for_Cross_Site_Script_Inclusion"),
    OTHER_ENTRY,
];

pub fn find(id: &str) -> Option<&'static Entry> {
    CATALOG.iter().find(|e| e.id == id)
}

/// Entries matching every word of `text` (id, title or `CWE-n`), any case.
pub fn search(text: &str) -> Vec<&'static Entry> {
    let words: Vec<String> = text.split_whitespace().map(str::to_lowercase).collect();
    CATALOG
        .iter()
        .filter(|e| {
            let hay = format!(
                "{} {} cwe-{}",
                e.id,
                e.title,
                e.cwe.map_or(String::new(), |c| c.to_string())
            )
            .to_lowercase();
            words.iter().all(|w| hay.contains(w.as_str()))
        })
        .collect()
}

/// The flux rules for a report's driver: those `ids` use, in catalog order.
pub fn flux_rules<'a>(ids: impl IntoIterator<Item = &'a str>) -> Vec<flux::Rule> {
    let used: std::collections::HashSet<&str> = ids.into_iter().collect();
    CATALOG
        .iter()
        .filter(|e| used.contains(e.id))
        .map(Entry::to_flux)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn is_wstg_id(s: &str) -> bool {
        let b = s.as_bytes();
        s.len() == 12
            && s.starts_with("WSTG-")
            && b[5..9].iter().all(u8::is_ascii_uppercase)
            && b[9] == b'-'
            && b[10..].iter().all(u8::is_ascii_digit)
    }

    #[test]
    fn ids_are_unique_wstg_ids_with_a_cwe_and_help() {
        let mut seen = std::collections::HashSet::new();
        for e in CATALOG {
            assert!(seen.insert(e.id), "duplicate {}", e.id);
            if e.id == OTHER {
                assert_eq!((e.cwe, e.help_uri()), (None, None));
                continue;
            }
            assert!(is_wstg_id(e.id), "{}", e.id);
            assert!(e.cwe.is_some_and(|c| c > 0), "{}", e.id);
            let help = e.help_uri().unwrap_or_default();
            assert!(
                help.starts_with(HELP_BASE) && !help.contains(' '),
                "{}",
                e.id
            );
            assert!(!e.title.is_empty());
        }
        assert!(CATALOG.len() > 50);
        assert_eq!(CATALOG.last().map(|e| e.id), Some(OTHER));
    }

    #[test]
    fn known_entries() {
        let sqli = find("WSTG-INPV-05").unwrap();
        assert_eq!(
            (sqli.title, sqli.cwe),
            ("Testing for SQL Injection", Some(89))
        );
        assert_eq!(
            sqli.help_uri().as_deref(),
            Some("https://owasp.org/www-project-web-security-testing-guide/v42/4-Web_Application_Security_Testing/07-Input_Validation_Testing/05-Testing_for_SQL_Injection")
        );
        assert_eq!(find("WSTG-ATHZ-04").and_then(|e| e.cwe), Some(639));
        assert_eq!(find("WSTG-CONF-12"), None, "CSP is not a v4.2 test");
        assert!(find(OTHER).is_some());
    }

    #[test]
    fn search_matches_every_word() {
        let ids = |t: &str| search(t).into_iter().map(|e| e.id).collect::<Vec<_>>();
        assert_eq!(ids("sql injection"), vec!["WSTG-INPV-05"]);
        assert_eq!(ids("cwe-639"), vec!["WSTG-ATHZ-04"]);
        assert_eq!(ids("athz-01"), vec!["WSTG-ATHZ-01"]);
        assert!(ids("cross site scripting").contains(&"WSTG-INPV-02"));
        assert!(ids("zzz").is_empty());
        assert_eq!(search("").len(), CATALOG.len());
    }

    #[test]
    fn flux_rules_are_valid_and_carry_their_cwe() {
        let rules = flux_rules(["WSTG-INPV-05", OTHER, "nope", "WSTG-INPV-01"]);
        assert_eq!(
            rules.iter().map(|r| r.id.as_str()).collect::<Vec<_>>(),
            ["WSTG-INPV-01", "WSTG-INPV-05", OTHER]
        );
        assert_eq!(rules[1].cwes(), vec![89]);
        assert!(rules[2].cwes().is_empty());
        let mut driver = flux::ToolComponent::tool("spall", "0.4.0");
        driver.rules = flux_rules(CATALOG.iter().map(|e| e.id));
        let report = flux::Report::new(driver, vec![]);
        assert_eq!(flux::validate::validate(&report), vec![]);
        assert!(report.runs[0].declares_taxonomy(flux::CWE));
    }
}
