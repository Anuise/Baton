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
    normalized.match_indices("token").any(|(at, _)| !normalized[at + "token".len()..].starts_with('s'))
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
        let next = words.get(index + 1).copied();
        word_looks_secret(word)
            || header_has_secret(word, next)
            || flag_has_secret(word, next)
            || ((word.eq_ignore_ascii_case("bearer") || word.eq_ignore_ascii_case("basic"))
                && next.is_some_and(|n| n.chars().count() >= 16 && !is_reference(n)))
    })
}

/// `--名稱 值` 這種旗標與值分成兩個字詞的寫法（例如 `--token abcd…`，或 args 陣列中相鄰的兩個元素）：
/// 旗標名稱像秘密，下一個字詞是夠長、不是另一個旗標、也不是環境變數參照的值。
pub(super) fn flag_has_secret(word: &str, next: Option<&str>) -> bool {
    let name = word.trim_start_matches('-');
    word.starts_with('-')
        && !name.is_empty()
        && !name.contains('=')
        && is_secret_key(name)
        && next.is_some_and(|value| !value.starts_with('-') && value.chars().count() >= 6 && !is_reference(value))
}

/// `名稱: 值` 或 `名稱:值` 的標頭形式（例如 `x-api-key: …`）：名稱像秘密，值是夠長、英數混合的 token。
fn header_has_secret(word: &str, next: Option<&str>) -> bool {
    let Some((name, value)) = word.split_once(':') else { return false };
    let value = if value.is_empty() { next.unwrap_or("") } else { value };
    is_secret_key(name)
        && value.chars().count() >= 16
        && value.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '/' | '+' | '='))
        && value.chars().any(|c| c.is_ascii_digit())
        && value.chars().any(|c| c.is_ascii_alphabetic())
}

fn word_looks_secret(word: &str) -> bool {
    // 金鑰可能緊貼在中文或其他字元之後（例如「備忘：ghp_…」、`--key=sk-ant-…`），
    // 所以在每個「前一個字元不是英數、底線或連字號」的位置都檢查一次
    let boundary = |at: usize| word[..at].chars().next_back().is_none_or(|c| !(c.is_ascii_alphanumeric() || c == '_' || c == '-'));
    if word.char_indices().any(|(at, _)| boundary(at) && known_token_shape(&word[at..])) {
        return true;
    }
    // `名稱=值`：名稱取等號前連續的英數、底線、連字號、點
    if let Some((before, value)) = word.split_once('=') {
        let key_start = before.rfind(|c: char| !(c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))).map_or(0, |i| i + before[i..].chars().next().map_or(1, char::len_utf8));
        let key = before[key_start..].trim_matches('-');
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

/// 把文字中看起來像秘密的片段換成 `replacement`，回傳遮蔽後的文字與遮蔽的數量（票 15：對話紀錄匯入）。
///
/// 辨識規則與 `contains_secret_value` 相同（已知金鑰格式、`名稱=值`、`名稱: 值`、`Bearer …`、含密碼的網址、私鑰區塊），
/// 另外把 64 個十六進位字元的片段視為秘密：Runtime 簽發的憑證就是這個形狀（代價是 SHA-256 雜湊也會被遮蔽）。
pub(crate) fn redact_secrets(text: &str, replacement: &str) -> (String, u64) {
    let mut count = 0;
    let mut text = text.to_string();
    // 私鑰區塊可能跨行，先整段處理
    while let Some(start) = text.find("-----BEGIN") {
        let Some(marker) = text[start..].find("PRIVATE KEY-----") else { break };
        let after_begin = start + marker + "PRIVATE KEY-----".len();
        let end = match text[after_begin..].find("PRIVATE KEY-----") {
            Some(offset) => after_begin + offset + "PRIVATE KEY-----".len(),
            None => text.len(),
        };
        text.replace_range(start..end, replacement);
        count += 1;
    }

    // 依分隔字元切成片段（保留分隔字元），逐一判斷
    let mut pieces: Vec<(bool, &str)> = Vec::new();
    let mut word_start = None;
    for (at, c) in text.char_indices() {
        if is_separator(c) {
            if let Some(start) = word_start.take() {
                pieces.push((true, &text[start..at]));
            }
            pieces.push((false, &text[at..at + c.len_utf8()]));
        } else if word_start.is_none() {
            word_start = Some(at);
        }
    }
    if let Some(start) = word_start {
        pieces.push((true, &text[start..]));
    }

    let words: Vec<usize> = pieces.iter().enumerate().filter(|(_, (is_word, _))| *is_word).map(|(i, _)| i).collect();
    let mut masked = vec![false; pieces.len()];
    for (index, &piece) in words.iter().enumerate() {
        let word = pieces[piece].1;
        if word_looks_secret(word) || has_token_hex(word) || header_has_secret(word, None) {
            masked[piece] = true;
            continue;
        }
        // 秘密在下一個片段：`Bearer <token>`、`名稱: <值>` 或 `--名稱 <值>`
        let Some(&next_piece) = words.get(index + 1) else { continue };
        let next = pieces[next_piece].1;
        let scheme = word.eq_ignore_ascii_case("bearer") || word.eq_ignore_ascii_case("basic");
        if (scheme && next.chars().count() >= 16 && !is_reference(next))
            || (word.ends_with(':') && header_has_secret(word, Some(next)))
            || flag_has_secret(word, Some(next))
        {
            masked[next_piece] = true;
        }
    }
    let mut out = String::with_capacity(text.len());
    for (index, (_, piece)) in pieces.iter().enumerate() {
        if masked[index] {
            out.push_str(replacement);
            count += 1;
        } else {
            out.push_str(piece);
        }
    }
    (out, count)
}

/// 含有恰好 64 個連續十六進位字元的片段（前後不是英數字元）。
fn has_token_hex(word: &str) -> bool {
    let bytes = word.as_bytes();
    let mut run = 0;
    for (at, b) in bytes.iter().enumerate() {
        if b.is_ascii_hexdigit() {
            run += 1;
            continue;
        }
        if run == 64 && !b.is_ascii_alphanumeric() && (at - run == 0 || !bytes[at - run - 1].is_ascii_alphanumeric()) {
            return true;
        }
        run = 0;
    }
    run == 64 && (bytes.len() == 64 || !bytes[bytes.len() - 65].is_ascii_alphanumeric())
}
