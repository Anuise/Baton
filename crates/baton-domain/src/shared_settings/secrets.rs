//! 登入秘密的辨識（只看形狀，不做網路查證）。
//!
//! 匯入共同設定時，任何看起來像登入資料的內容都不能進入版本快照。辨識分兩種：
//! 一是「欄位名稱像秘密」（例如 `GITHUB_TOKEN`、`Authorization`），二是「字串內容像秘密」（例如 `sk-ant-…`、
//! `ghp_…`、JWT、`Bearer …`、含密碼的 URL）。這是啟發式規則，不保證抓得到所有秘密；
//! 因此另有明確的來源白名單（只讀特定檔案的特定欄位）與排除清單，這裡是最後一道網。

/// 切字用的分隔字元：空白與常見的引號、括號、逗號。
fn is_separator(c: char) -> bool {
    c.is_whitespace() || matches!(c, '"' | '\'' | ',' | ';' | '(' | ')' | '[' | ']' | '{' | '}' | '<' | '>' | '`')
}

/// 欄位名稱看起來是秘密（呼叫端只對「非空字串值」套用）。
///
/// `tokens`（複數，例如 `MAX_OUTPUT_TOKENS`）是用量上限，不是秘密。
pub(super) fn is_secret_key(key: &str) -> bool {
    let normalized: String = key.chars().filter(|c| !matches!(c, '-' | '_' | ' ' | '.')).collect::<String>().to_lowercase();
    const NAMES: [&str; 10] = [
        "apikey",
        "secret",
        "password",
        "passwd",
        "credential",
        "authorization",
        "privatekey",
        "cookie",
        "accesskey",
        "sessionkey",
    ];
    if NAMES.iter().any(|name| normalized.contains(name)) {
        return true;
    }
    normalized.match_indices("token").any(|(at, _)| normalized[at + "token".len()..].chars().next() != Some('s'))
}

/// `settings.json` 的 `env` 中會改變認證來源的變數（AI-78：覆蓋會讓 Profile 的原生登入失效）。
pub(super) fn is_login_env_key(key: &str) -> bool {
    let upper = key.to_uppercase();
    upper.starts_with("ANTHROPIC_")
        || upper.starts_with("CLAUDE_CODE_OAUTH")
        || upper.starts_with("CLAUDE_CODE_USE_")
        || upper.starts_with("AWS_")
        || upper.starts_with("GOOGLE_APPLICATION_CREDENTIALS")
}

/// 值只是環境變數參照（`${NAME}`，可帶 `Bearer ` 前綴）：它不含秘密本身，可以保留。
pub(super) fn is_reference(value: &str) -> bool {
    let value = value.trim();
    let value = match value.split_once(' ') {
        Some((scheme, rest)) if scheme.eq_ignore_ascii_case("bearer") || scheme.eq_ignore_ascii_case("basic") => rest.trim(),
        _ => value,
    };
    value.starts_with("${") && value.ends_with('}') && value.matches('$').count() == 1
}

/// 字串內容含有看起來像秘密的片段。
pub(crate) fn contains_secret_value(text: &str) -> bool {
    if text.contains("-----BEGIN") && text.contains("PRIVATE KEY-----") {
        return true;
    }
    let words: Vec<&str> = text.split(is_separator).filter(|w| !w.is_empty()).collect();
    words.iter().enumerate().any(|(index, word)| {
        word_looks_secret(word) || {
            let next = words.get(index + 1).copied();
            (word.eq_ignore_ascii_case("bearer") || word.eq_ignore_ascii_case("basic"))
                && next.is_some_and(|n| n.chars().count() >= 16 && !is_reference(n))
        }
    })
}

fn word_looks_secret(word: &str) -> bool {
    // 例如 `--key=sk-ant-…`：整個字、以及每個 `=` 之後的部分都要檢查
    let tails = std::iter::once(word).chain(word.match_indices('=').map(|(at, _)| &word[at + 1..]));
    for tail in tails {
        if known_token_shape(tail.trim_start_matches(|c: char| !c.is_alphanumeric())) {
            return true;
        }
    }
    if let Some((key, value)) = word.split_once('=') {
        let key = key.trim_matches('-');
        if is_secret_key(key) && value.chars().count() >= 6 && !is_reference(value) {
            return true;
        }
    }
    url_has_password(word)
}

/// 常見服務的金鑰格式：已知前綴加上足夠長的內容。
fn known_token_shape(s: &str) -> bool {
    let run = |rest: &str, accept: fn(char) -> bool| rest.chars().take_while(|c| accept(*c)).count();
    let word_char: fn(char) -> bool = |c| c.is_ascii_alphanumeric() || c == '-' || c == '_';
    const PREFIXES: [(&str, usize); 16] = [
        ("sk-ant-", 8),
        ("sk-", 20),
        ("ghp_", 20),
        ("gho_", 20),
        ("ghu_", 20),
        ("ghs_", 20),
        ("ghr_", 20),
        ("github_pat_", 20),
        ("glpat-", 20),
        ("xoxb-", 10),
        ("xoxa-", 10),
        ("xoxp-", 10),
        ("xoxs-", 10),
        ("sk_live_", 16),
        ("rk_live_", 16),
        ("pk_live_", 16),
    ];
    if PREFIXES.iter().any(|(prefix, min)| s.starts_with(prefix) && run(&s[prefix.len()..], word_char) >= *min) {
        return true;
    }
    if (s.starts_with("AKIA") || s.starts_with("ASIA")) && run(&s[4..], |c| c.is_ascii_uppercase() || c.is_ascii_digit()) >= 16 {
        return true;
    }
    if s.starts_with("AIza") && run(&s[4..], word_char) >= 35 {
        return true;
    }
    if s.starts_with("eyJ") {
        let parts: Vec<&str> = s.split('.').collect();
        if parts.len() >= 3 && parts[..3].iter().all(|p| p.len() >= 8) {
            return true;
        }
    }
    false
}

/// `scheme://user:password@host` 這種把密碼寫進網址的形式。
fn url_has_password(word: &str) -> bool {
    let Some((_, rest)) = word.split_once("://") else { return false };
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    let Some((userinfo, _)) = authority.rsplit_once('@') else { return false };
    userinfo.split_once(':').is_some_and(|(_, password)| !password.is_empty() && !is_reference(password))
}
