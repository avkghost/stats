pub(crate) fn referrer_domain(referer: Option<&str>) -> Option<String> {
    let referer = referer?.trim();
    let (_, authority) = referer.split_once("://")?;
    let authority = authority.split(['/', '?', '#']).next()?;
    let authority = authority
        .rsplit_once('@')
        .map_or(authority, |(_, host)| host);
    let host = if authority.starts_with('[') {
        authority.strip_prefix('[')?.split(']').next()?
    } else if authority.matches(':').count() == 1 {
        authority.split(':').next()?
    } else {
        authority
    };
    let host = host.trim().to_ascii_lowercase();
    (!host.is_empty()).then_some(host)
}

pub(super) fn stable_key(parts: &[&str]) -> String {
    let mut hash = 0xcbf29ce484222325u64;
    for part in parts {
        for byte in part.as_bytes() {
            hash ^= u64::from(*byte);
            hash = hash.wrapping_mul(0x100000001b3);
        }
        hash ^= 0xff;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("{hash:016x}")
}

pub(super) fn visitor_label(client_ip: &str, client_hostname: Option<&str>) -> String {
    let ip = client_ip.trim();
    match client_hostname
        .map(str::trim)
        .filter(|hostname| !hostname.is_empty() && *hostname != "-")
    {
        Some(hostname) => format!("{ip} ({hostname})"),
        None => ip.to_owned(),
    }
}

pub(super) fn file_type(url: Option<&str>) -> String {
    let path = url
        .unwrap_or("UNKNOWN")
        .split('?')
        .next()
        .unwrap_or("UNKNOWN");
    let Some(extension) = path
        .rsplit('/')
        .next()
        .and_then(|name| name.rsplit_once('.'))
        .map(|(_, ext)| ext)
    else {
        return "(none)".to_owned();
    };
    if extension.is_empty()
        || extension.len() > 16
        || !extension.chars().all(|c| c.is_ascii_alphanumeric())
    {
        "(none)".to_owned()
    } else {
        format!(".{}", extension.to_ascii_lowercase())
    }
}

pub(super) fn search_referrer(referer: Option<&str>) -> Option<(&'static str, String)> {
    let referer = referer?;
    let (_, rest) = referer.split_once("://")?;
    let authority = rest.split(['/', '?']).next()?.to_ascii_lowercase();
    let engine = [
        ("google.", "Google"),
        ("bing.com", "Bing"),
        ("duckduckgo.com", "DuckDuckGo"),
        ("yandex.", "Yandex"),
        ("search.brave.com", "Brave"),
    ]
    .iter()
    .find_map(|(needle, label)| authority.contains(needle).then_some(*label))?;
    let query = referer.split_once('?')?.1;
    let phrase = query
        .split('&')
        .filter_map(|part| part.split_once('='))
        .find(|(key, _)| matches!(*key, "q" | "query" | "p" | "text"))
        .map(|(_, value)| decode_query(value))
        .unwrap_or_default();
    Some((engine, phrase.chars().take(256).collect()))
}

pub(super) fn decode_query(value: &str) -> String {
    let mut decoded = Vec::with_capacity(value.len());
    let bytes = value.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'+' => {
                decoded.push(b' ');
                index += 1;
            }
            b'%' if index + 2 < bytes.len() => {
                let high = (bytes[index + 1] as char).to_digit(16);
                let low = (bytes[index + 2] as char).to_digit(16);
                if let (Some(high), Some(low)) = (high, low) {
                    decoded.push((high * 16 + low) as u8);
                    index += 3;
                } else {
                    return value.to_owned();
                }
            }
            b'%' => return value.to_owned(),
            byte => {
                decoded.push(byte);
                index += 1;
            }
        }
    }
    String::from_utf8(decoded).unwrap_or_else(|_| value.to_owned())
}

pub(super) fn browser(user_agent: &str) -> &'static str {
    let ua = user_agent.to_ascii_lowercase();
    [
        ("edg/", "Edge"),
        ("opr/", "Opera"),
        ("opera", "Opera"),
        ("firefox/", "Firefox"),
        ("chrome/", "Chrome"),
        ("chromium/", "Chrome"),
        ("safari/", "Safari"),
        ("curl/", "curl"),
        ("wget/", "Wget"),
    ]
    .iter()
    .find_map(|(needle, label)| ua.contains(needle).then_some(*label))
    .unwrap_or("Other")
}

pub(super) fn operating_system(user_agent: &str) -> &'static str {
    let ua = user_agent.to_ascii_lowercase();
    [
        ("windows", "Windows"),
        ("android", "Android"),
        ("iphone", "iOS"),
        ("ipad", "iOS"),
        ("ios", "iOS"),
        ("mac os", "macOS"),
        ("macintosh", "macOS"),
        ("linux", "Linux"),
    ]
    .iter()
    .find_map(|(needle, label)| ua.contains(needle).then_some(*label))
    .unwrap_or("Other")
}

pub(super) fn robot(user_agent: &str) -> Option<&'static str> {
    let ua = user_agent.to_ascii_lowercase();
    [
        ("googlebot", "Googlebot"),
        ("bingbot", "Bingbot"),
        ("gptbot", "GPTBot"),
        ("crawler", "Crawler"),
        ("spider", "Spider"),
        ("bot", "Bot"),
        ("curl/", "curl"),
        ("wget/", "Wget"),
    ]
    .iter()
    .find_map(|(needle, label)| ua.contains(needle).then_some(*label))
}

pub(super) fn request_time_bucket(seconds: f64) -> &'static str {
    if seconds < 0.01 {
        "<10ms"
    } else if seconds < 0.1 {
        "10-100ms"
    } else if seconds < 0.5 {
        "100-500ms"
    } else if seconds < 1.0 {
        "500ms-1s"
    } else if seconds < 5.0 {
        "1-5s"
    } else {
        ">=5s"
    }
}
