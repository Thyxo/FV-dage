//! Parser til svaret på `/ce info <celle>`. Eksempel fra serveren:
//!
//! ```text
//! ------------={ a142 }=------------
//! Ejer: Thyxo
//! Blok: a
//! Tid: 7 dage, 20 timer, 20 minutter
//! Medlemmer:
//! ```

use regex::Regex;
use serde::{Deserialize, Serialize};
use std::sync::OnceLock;

const MIN_PER_DAY: u32 = 24 * 60;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CeInfo {
    pub cell: String,
    pub owner: Option<String>,
    pub block: Option<String>,
    pub days: u32,
    pub hours: u32,
    pub minutes: u32,
    /// Teksten efter "Tid:", fx "7 dage, 20 timer, 20 minutter".
    pub time_text: String,
}

impl CeInfo {
    pub fn total_minutes(&self) -> u32 {
        self.days * MIN_PER_DAY + self.hours * 60 + self.minutes
    }

    /// Dage rundet op: 8 dage 23 timer tæller som 9.
    pub fn days_rounded_up(&self) -> u32 {
        self.total_minutes().div_ceil(MIN_PER_DAY)
    }


    /// Hvor mange klik (1 klik = 1 dag) der mangler for at nå `max_days`.
    pub fn clicks_needed(&self, max_days: u8) -> u32 {
        u32::from(max_days).saturating_sub(self.days_rounded_up())
    }

    /// Værdi til cell-manager (0-9).
    pub fn manager_days(&self) -> u8 {
        self.days_rounded_up().min(9) as u8
    }
}

/// Regex der matcher overskriften for en bestemt celle.
pub fn header_regex(cell: &str) -> String {
    format!(r"=\{{\s*{}\s*\}}=", regex::escape(cell.trim()))
}

pub const END_REGEX: &str = r"Medlemmer\s*:";

fn re(cell: &'static OnceLock<Regex>, pattern: &str) -> &'static Regex {
    cell.get_or_init(|| Regex::new(pattern).expect("gyldig regex"))
}

/// Finder /ce info-svaret blandt chatlinjerne. Andre beskeder ignoreres.
/// Med `expected_cell` afvises svar for en anden celle.
pub fn parse(lines: &[String], expected_cell: Option<&str>) -> Result<CeInfo, String> {
    static HEADER: OnceLock<Regex> = OnceLock::new();
    static DAYS: OnceLock<Regex> = OnceLock::new();
    static HOURS: OnceLock<Regex> = OnceLock::new();
    static MINUTES: OnceLock<Regex> = OnceLock::new();
    let header = re(&HEADER, r"=\{\s*([^}\s]+)\s*\}=");

    let start = lines
        .iter()
        .position(|l| header.is_match(l))
        .ok_or_else(|| "Fandt ikke /ce info-overskriften i chatten".to_string())?;
    let cell = header.captures(&lines[start]).unwrap()[1].to_string();
    if let Some(expected) = expected_cell {
        if !cell.eq_ignore_ascii_case(expected.trim()) {
            return Err(format!("Svaret handlede om celle {cell}, ikke {expected}"));
        }
    }

    let mut info = CeInfo {
        cell,
        owner: None,
        block: None,
        days: 0,
        hours: 0,
        minutes: 0,
        time_text: String::new(),
    };
    let mut found_time = false;

    for line in &lines[start + 1..] {
        if header.is_match(line) {
            break; // næste /ce info-svar
        }
        if let Some(v) = value_after(line, "Ejer:") {
            info.owner = Some(v);
        } else if let Some(v) = value_after(line, "Blok:") {
            info.block = Some(v);
        } else if let Some(v) = value_after(line, "Tid:") {
            let num = |r: &Regex| r.captures(&v).and_then(|c| c[1].parse().ok()).unwrap_or(0);
            info.days = num(re(&DAYS, r"(?i)(\d+)\s*dag"));
            info.hours = num(re(&HOURS, r"(?i)(\d+)\s*time"));
            info.minutes = num(re(&MINUTES, r"(?i)(\d+)\s*min"));
            info.time_text = v;
            found_time = true;
        }
    }

    if !found_time {
        return Err("Fandt ingen \"Tid:\"-linje i /ce info-svaret".into());
    }
    Ok(info)
}

/// Teksten efter fx "Ejer:", uanset hvad der står foran (tidsstempler, pile osv.).
fn value_after(line: &str, key: &str) -> Option<String> {
    line.find(key).map(|i| line[i + key.len()..].trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(s: &str) -> Vec<String> {
        s.lines().map(String::from).collect()
    }

    #[test]
    fn parser_eksempel_fra_serveren() {
        let l = lines("Spiller1: hej\n------------={ a142 }=------------\nEjer: Thyxo\nBlok: a\nTid: 7 dage, 20 timer, 20 minutter\nMedlemmer: ");
        let i = parse(&l, Some("A142")).unwrap();
        assert_eq!((i.days, i.hours, i.minutes), (7, 20, 20));
        assert_eq!(i.owner.as_deref(), Some("Thyxo"));
        assert_eq!(i.days_rounded_up(), 8);
        assert_eq!(i.clicks_needed(9), 1);
    }

    #[test]
    fn afviser_forkert_celle_og_manglende_tid() {
        let l = lines("={ b1 }=\nTid: 1 dag");
        assert!(parse(&l, Some("a142")).is_err());
        assert!(parse(&lines("={ a1 }=\nEjer: x"), Some("a1")).is_err());
    }

    #[test]
    fn fuld_celle_og_udloebet() {
        let full = parse(&lines("={ a1 }=\nTid: 8 dage, 23 timer, 59 minutter"), Some("a1")).unwrap();
        assert_eq!(full.manager_days(), 9);
        let zero = parse(&lines("={ a1 }=\nTid: Udløbet"), Some("a1")).unwrap();
        assert_eq!(zero.clicks_needed(9), 9);
    }
}
