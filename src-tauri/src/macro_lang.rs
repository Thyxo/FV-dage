//! Makro-sproget. Kompatibelt med headless-client, plus `ceinfo` og `fyldop`.
//!
//! Én kommando pr. linje, `#` = kommentar. Variabler: `{celle}`, `{konto}`,
//! `{max}` og `{antal}` (klik der mangler, ud fra seneste /ce info).

#[derive(Clone, Debug, PartialEq)]
pub enum Validation {
    Contains(String),
    Regex(String),
}

#[derive(Clone, Debug, PartialEq)]
pub enum Step {
    Start,
    WaitJoin(u64),
    JoinMacro,
    Wait(u64),
    Chat(String),
    Hotbar(u8),
    Sign { clicks: Option<u32>, delay_ms: Option<u64>, max_distance: Option<u32> },
    CheckChat { timeout_ms: Option<u64>, command: String },
    ValidateChat(Validation),
    Click { slot: i32, button: i32, mode: i32 },
    /// forward/back/left/right/jump/sneak/sprint i et antal ms.
    Move { action: String, ms: u64 },
    /// stopmove, swing, use, deactivate, drop
    Action(String),
    /// `/ce info <mål>` (standard `{celle}`), gemmes som aflæsning.
    CeInfo(Option<String>),
    /// Klik til cellen er fuld og verificér med /ce info.
    Fill { rounds: Option<u8> },
}

pub struct Vars<'a> {
    pub cell: &'a str,
    pub account: &'a str,
    pub max_days: u8,
    pub clicks_needed: u32,
}

pub fn expand(line: &str, v: &Vars) -> String {
    line.replace("{celle}", v.cell)
        .replace("{konto}", v.account)
        .replace("{max}", &v.max_days.to_string())
        .replace("{antal}", &v.clicks_needed.to_string())
}

/// Linjer der skal køres (uden tomme linjer og kommentarer), med linjenummer.
pub fn lines(script: &str) -> Vec<(usize, String)> {
    script
        .lines()
        .enumerate()
        .map(|(i, l)| (i + 1, l.trim().to_string()))
        .filter(|(_, l)| !l.is_empty() && !l.starts_with('#'))
        .collect()
}

fn num<T: std::str::FromStr>(arg: Option<&str>, what: &str) -> Result<Option<T>, String> {
    match arg {
        None => Ok(None),
        Some(a) => a
            .parse()
            .map(Some)
            .map_err(|_| format!("{what} skal være et tal (fik \"{a}\")")),
    }
}

fn rest_after_command(line: &str) -> String {
    line.split_once(char::is_whitespace)
        .map(|(_, r)| r.trim().to_string())
        .unwrap_or_default()
}

pub fn parse_line(line: &str) -> Result<Step, String> {
    let parts: Vec<&str> = line.split_whitespace().collect();
    let Some(cmd) = parts.first() else {
        return Err("Tom linje".into());
    };
    let cmd = cmd.to_lowercase();
    let arg = |i: usize| parts.get(i).copied();

    let step = match cmd.as_str() {
        "start" => Step::Start,
        "waitjoin" => Step::WaitJoin(num(arg(1), "waitjoin")?.unwrap_or(30_000)),
        "joinmacro" => Step::JoinMacro,
        "wait" => Step::Wait(num(arg(1), "wait")?.unwrap_or(500)),
        "chat" => {
            let text = rest_after_command(line);
            if text.is_empty() {
                return Err("chat kræver en tekst".into());
            }
            Step::Chat(text)
        }
        "hotbar" => {
            let slot: u8 = num(arg(1), "hotbar")?.unwrap_or(1);
            if !(1..=9).contains(&slot) {
                return Err("hotbar skal være 1-9".into());
            }
            Step::Hotbar(slot)
        }
        "sign" => Step::Sign {
            clicks: num(arg(1), "antal klik")?,
            delay_ms: num(arg(2), "delay")?,
            max_distance: num(arg(3), "afstand")?,
        },
        "checkchat" => {
            // checkchat [timeoutMs] <kommando>
            let (timeout_ms, command) = match arg(1).map(|a| a.parse::<u64>()) {
                Some(Ok(t)) => (Some(t), parts[2..].join(" ")),
                _ => (None, parts[1..].join(" ")),
            };
            if command.is_empty() {
                return Err("checkchat kræver en kommando".into());
            }
            Step::CheckChat { timeout_ms, command }
        }
        "validatechat" => {
            let kind = arg(1).map(str::to_lowercase);
            let value = parts.get(2..).map(|p| p.join(" ")).unwrap_or_default();
            match (kind.as_deref(), value.is_empty()) {
                (Some("contains"), false) => Step::ValidateChat(Validation::Contains(value)),
                (Some("regex"), false) => {
                    regex::Regex::new(&value).map_err(|e| format!("Ugyldig regex: {e}"))?;
                    Step::ValidateChat(Validation::Regex(value))
                }
                _ => return Err("validatechat kræver: contains <tekst> eller regex <mønster>".into()),
            }
        }
        "click" => Step::Click {
            slot: num(arg(1), "slot")?.ok_or("click kræver et slot-nummer")?,
            button: num(arg(2), "knap")?.unwrap_or(0),
            mode: num(arg(3), "mode")?.unwrap_or(0),
        },
        "forward" | "back" | "left" | "right" | "jump" | "sneak" | "sprint" => Step::Move {
            action: cmd.clone(),
            ms: num(arg(1), &cmd)?.unwrap_or(350).min(30_000),
        },
        "stopmove" | "stop" => Step::Action("stopmove".into()),
        "swing" | "use" | "deactivate" | "drop" => Step::Action(cmd.clone()),
        "ceinfo" => Step::CeInfo(arg(1).map(String::from)),
        "fyldop" | "fill" => Step::Fill { rounds: num(arg(1), "runder")? },
        other => return Err(format!("Ukendt kommando: {other}")),
    };
    Ok(step)
}

/// Tjekker en hel makro og returnerer fejl med linjenumre.
pub fn validate(script: &str) -> Vec<String> {
    let vars = Vars { cell: "a1", account: "Steve", max_days: 9, clicks_needed: 9 };
    lines(script)
        .into_iter()
        .filter_map(|(n, l)| parse_line(&expand(&l, &vars)).err().map(|e| format!("Linje {n}: {e}")))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gammel_makro_fra_headless_client_virker() {
        let old = "start\nwaitjoin 30000\njoinmacro\nsign 30 300 16\ncheckchat 5000 /ce info\nvalidatechat contains 30";
        assert!(validate(old).is_empty());
        assert_eq!(
            parse_line("checkchat 5000 /ce info a1").unwrap(),
            Step::CheckChat { timeout_ms: Some(5000), command: "/ce info a1".into() }
        );
        assert_eq!(parse_line("stop").unwrap(), Step::Action("stopmove".into()));
    }

    #[test]
    fn variabler_og_fejl() {
        let v = Vars { cell: "a142", account: "Thyxo", max_days: 9, clicks_needed: 2 };
        assert_eq!(expand("sign {antal} 300 8", &v), "sign 2 300 8");
        assert_eq!(validate("# kommentar\nhop 3\nhotbar 12").len(), 2);
    }
}
