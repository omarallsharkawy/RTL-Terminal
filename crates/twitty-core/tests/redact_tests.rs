use std::borrow::Cow;
use std::time::Instant;

use twitty_core::redact::redact;

#[test]
fn table_driven_secret_redaction() {
    let cases: &[(&str, &str, &str)] = &[
        (
            "github_ghp",
            "git clone https://ghp_1234567890abcdef1234567890abcdef1234@github.com/org/repo",
            "git clone https://[REDACTED:github_token]@github.com/org/repo",
        ),
        (
            "github_gho",
            "token gho_abcdefghijklmnopqrstuvwxyz1234567890 done",
            "token [REDACTED:github_token] done",
        ),
        (
            "github_ghu",
            "ghu_ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789",
            "[REDACTED:github_token]",
        ),
        (
            "github_ghs",
            "ghs_0123456789abcdefghijklmnopqrstuvwxyz",
            "[REDACTED:github_token]",
        ),
        (
            "github_ghr",
            "ghr_0123456789abcdefghijklmnopqrstuvwxyz",
            "[REDACTED:github_token]",
        ),
        (
            "github_pat",
            "export X=github_pat_11AAAAAA0123456789abcd_ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789abcdef012345",
            "export X=[REDACTED:github_token]",
        ),
        (
            "openai_legacy",
            concat!("curl -d sk-", "AbCdEfGhIjKlMnOpQrStT3BlbkFJ1234567890AbCdEfGhIj"),
            "curl -d [REDACTED:openai_key]",
        ),
        (
            "openai_proj",
            "key=sk-proj-abcdefghijklmnopqrstuvwxyz0123456789_ABC-def",
            "key=[REDACTED:openai_key]",
        ),
        (
            "anthropic",
            "use sk-ant-api03-abcdefghijklmnopqrstuvwxyz0123456789_-AA",
            "use [REDACTED:anthropic_key]",
        ),
        (
            "google_api_key",
            "https://maps.googleapis.com/maps/api/js?key=AIzaSyDaGmWKa4JsXZ-HjGw7ISLn_3namBGewQe",
            "https://maps.googleapis.com/maps/api/js?key=[REDACTED:google_api_key]",
        ),
        (
            "aws_akia",
            "access key AKIAIOSFODNN7EXAMPLE in log",
            "access key [REDACTED:aws_access_key] in log",
        ),
        (
            "aws_asia",
            "sts ASIAQ3EG7EXAMPLE1234",
            "sts [REDACTED:aws_access_key]",
        ),
        (
            "aws_secret_ini",
            "[default]\naws_access_key_id = AKIAIOSFODNN7EXAMPLE\naws_secret_access_key = wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY\n",
            "[default]\naws_access_key_id = [REDACTED:aws_access_key]\naws_secret_access_key = [REDACTED:aws_secret_key]\n",
        ),
        (
            "slack_bot",
            concat!("slack xox", "b-123456789012-1234567890123-aBcDeFgHiJkLmNoPqRsTuVwX"),
            "slack [REDACTED:slack_token]",
        ),
        (
            "slack_user",
            concat!("xox", "p-1111111111-2222222222-3333333333-abcdef123456"),
            "[REDACTED:slack_token]",
        ),
        (
            "slack_other_types",
            "xoxa-2-1234567890abcdef xoxr-1234567890abcdef xoxs-1234567890abcdef",
            "[REDACTED:slack_token] [REDACTED:slack_token] [REDACTED:slack_token]",
        ),
        (
            "stripe_sk_live",
            concat!("stripe sk_", "live_51H7xYzAbCdEfGhIjKlMnOpQrStUvWx"),
            "stripe [REDACTED:stripe_key]",
        ),
        (
            "stripe_rk_live",
            concat!("stripe rk_", "live_51H7xYzAbCdEfGhIjKlMnOpQrStUvWx"),
            "stripe [REDACTED:stripe_key]",
        ),
        (
            "jwt",
            "cookie=eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJzdWIiOiIxMjM0NTY3ODkwIiwibmFtZSI6IkFsaSJ9.SflKxwRJSMeKKF2QT4fwpMeJf36POk6yJV_adQssw5c;",
            "cookie=[REDACTED:jwt];",
        ),
        (
            "auth_bearer",
            "curl -H 'Authorization: Bearer super_secret_bearer_token_99' https://api.example.com",
            "curl -H 'Authorization: Bearer [REDACTED:auth_header]' https://api.example.com",
        ),
        (
            "auth_basic_case_insensitive",
            "authorization: basic dXNlcjpwYXNz",
            "authorization: basic [REDACTED:auth_header]",
        ),
        (
            "proxy_auth",
            "Proxy-Authorization: Basic dXNlcjpwYXNz",
            "Proxy-Authorization: Basic [REDACTED:auth_header]",
        ),
        (
            "url_password_postgres",
            "psql postgres://alice:p4ssw0rd!@db.internal:5432/prod?sslmode=require",
            "psql postgres://alice:[REDACTED:url_password]@db.internal:5432/prod?sslmode=require",
        ),
        (
            "url_password_https",
            "git clone https://omar:my-secret-pass@git.example.com/repo.git",
            "git clone https://omar:[REDACTED:url_password]@git.example.com/repo.git",
        ),
        (
            "env_file_lines",
            "PORT=8080\nAPI_KEY=supersecret\nDB_PASSWORD=\"p@ss word\"\nexport MY_SECRET='abc123'\nPRIVATE_KEY=xyz\nUSER_CREDENTIAL=cred99\nFTP_PASSWD=fpw\nAUTH_TOKEN=tok123 # comment\n",
            "PORT=8080\nAPI_KEY=[REDACTED:env_secret]\nDB_PASSWORD=\"[REDACTED:env_secret]\"\nexport MY_SECRET='[REDACTED:env_secret]'\nPRIVATE_KEY=[REDACTED:env_secret]\nUSER_CREDENTIAL=[REDACTED:env_secret]\nFTP_PASSWD=[REDACTED:env_secret]\nAUTH_TOKEN=[REDACTED:env_secret] # comment\n",
        ),
        (
            "inline_env_before_cmd",
            "PGPASSWORD=topsecret psql -h localhost -U admin appdb",
            "PGPASSWORD=[REDACTED:env_secret] psql -h localhost -U admin appdb",
        ),
        (
            "cli_password_flag_equals",
            "mysql -u root --password=hunter2 appdb",
            "mysql -u root --password=[REDACTED:cli_password] appdb",
        ),
        (
            "cli_password_flag_space",
            "mysqldump -u root --password 'my secret' appdb",
            "mysqldump -u root --password '[REDACTED:cli_password]' appdb",
        ),
        (
            "mysql_short_p_attached",
            "mysql -u root -phunter2 appdb",
            "mysql -u root -p[REDACTED:cli_password] appdb",
        ),
        (
            "mariadb_short_p_with_path",
            "/usr/bin/mariadb -h 127.0.0.1 -u admin -p's3cr3t!' db",
            "/usr/bin/mariadb -h 127.0.0.1 -u admin -p'[REDACTED:cli_password]' db",
        ),
        (
            "psql_long_password_flag",
            "psql --password=mysecret -U postgres",
            "psql --password=[REDACTED:cli_password] -U postgres",
        ),
        (
            "pem_private_key_block",
            "before\n-----BEGIN RSA PRIVATE KEY-----\nMIIEpAIBAAKCAQEA0Z3VS5JJcds3xfn/ygWyF8PbnGy0AHB7Mhgw\nmorebase64lines==\n-----END RSA PRIVATE KEY-----\nafter",
            "before\n[REDACTED:pem_private_key]\nafter",
        ),
        (
            "pem_openssh_private_key",
            "-----BEGIN OPENSSH PRIVATE KEY-----\nb3BlbnNzaC1rZXktdjEAAAAABG5vbmU=\n-----END OPENSSH PRIVATE KEY-----",
            "[REDACTED:pem_private_key]",
        ),
        (
            "pem_truncated_private_key_without_end",
            "before\n-----BEGIN RSA PRIVATE KEY-----\nMIIEpAIBAAKCAQEA0Z3VS5JJcds3xfn/ygWyF8PbnGy0AHB7Mhgw\nmorebase64lines==",
            "before\n[REDACTED:pem_private_key]",
        ),
        (
            "arabic_around_secret",
            "تشغيل الأمر مع مفتاح ghp_1234567890abcdef1234567890abcdef1234 في المشروع",
            "تشغيل الأمر مع مفتاح [REDACTED:github_token] في المشروع",
        ),
        (
            "arabic_with_url_password",
            "اتصال بقاعدة البيانات postgres://عمر:secret123@localhost/db تم بنجاح",
            "اتصال بقاعدة البيانات postgres://عمر:[REDACTED:url_password]@localhost/db تم بنجاح",
        ),
    ];

    for &(name, input, expected) in cases {
        let actual = redact(input);
        assert_eq!(actual.as_ref(), expected, "case {name:?} failed");
        assert!(
            matches!(actual, Cow::Owned(_)),
            "case {name:?} should allocate only when redacting"
        );
        // Redacting an already-redacted string must be idempotent and zero-alloc.
        let second = redact(actual.as_ref());
        assert_eq!(second.as_ref(), expected, "case {name:?} not idempotent");
        assert!(
            matches!(second, Cow::Borrowed(_)),
            "case {name:?} second pass should borrow"
        );
    }
}

#[test]
fn negatives_stay_untouched_and_zero_alloc() {
    let negatives: &[(&str, &str)] = &[
        ("empty", ""),
        ("short", "ls -la"),
        (
            "arabic_text_with_harakat",
            "بِسْمِ اللَّهِ — تشغيل الاختبارات في مجلد المشروع 2026 بنجاح ✓",
        ),
        (
            "git_sha1_40",
            "git show 7abaf40e1234567890abcdef1234567890abcdef",
        ),
        (
            "git_sha256_64",
            "sha256: e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
        ),
        (
            "uuids",
            "id=550e8400-e29b-41d4-a716-446655440000 trace=f47ac10b-58cc-4372-a567-0e02b2c3d479",
        ),
        (
            "ordinary_base64",
            "echo SGVsbG8gd29ybGQgdGhpcyBpcyBub3QgYSBzZWNyZXQga2V5IGp1c3QgYmFzZTY0 | base64 -d",
        ),
        (
            "public_pem_certificate",
            "-----BEGIN PUBLIC KEY-----\nMIIBIjANBgkqhkiG9w0BAQEFAAOCAQ8A\n-----END PUBLIC KEY-----",
        ),
        (
            "stripe_publishable_key",
            "const key = 'pk_live_51H7xYzAbCdEfGhIjKlMnOpQrStUvWx';",
        ),
        (
            "url_without_password",
            "https://example.com/path?foo=bar&baz=123",
        ),
        (
            "ssh_git_remote_no_scheme",
            "git clone git@github.com:omarallsharkawy/RTL-Terminal.git",
        ),
        (
            "psql_port_flag_not_password",
            "psql -h localhost -p5432 -U postgres mydb",
        ),
        (
            "mysql_interactive_p_with_space",
            "mysql -u root -p mydb",
        ),
        (
            "ssh_and_mkdir_short_p",
            "ssh -p2222 user@host && mkdir -p/tmp/demo",
        ),
        (
            "env_var_reference_not_secret",
            "export API_KEY=$MY_KEY && curl -H \"Authorization: Bearer $TOKEN\" https://api.example.com",
        ),
        (
            "equality_comparison_in_code",
            "if API_KEY == expected_value { ok(); }",
        ),
        (
            "hyphenated_words_starting_sk",
            "sk-something-without-any-digits-in-this-name",
        ),
        (
            "normal_env_vars",
            "RUST_LOG=debug CARGO_TARGET_DIR=/tmp/twitty-target LANG=ar_EG.UTF-8",
        ),
    ];

    for &(name, input) in negatives {
        let actual = redact(input);
        assert_eq!(actual.as_ref(), input, "negative case {name:?} was mangled");
        assert!(
            matches!(actual, Cow::Borrowed(_)),
            "negative case {name:?} should return Cow::Borrowed"
        );
    }
}

#[test]
fn linear_time_on_five_megabyte_adversarial_input() {
    // 5 MiB mixing Arabic, near-miss prefixes, repeated `://`, `eyJ.`, `sk-`, `-----BEGIN `,
    // and real secrets at the end.
    let chunk = "مرحبا بالعالم https://example.com/a:b eyJshort.notjwt sk-nope-short \
                 -----BEGIN NOT PRIVATE----- a://a://a:// PASSWORD==foo 7abaf40deadbeef0123456789abcdef012345678\n";
    let target_bytes = 5 * 1024 * 1024;
    let mut big = String::with_capacity(target_bytes + 256);
    while big.len() < target_bytes {
        big.push_str(chunk);
    }
    big.push_str("tail ghp_1234567890abcdef1234567890abcdef1234 end");

    let start = Instant::now();
    let redacted = redact(&big);
    let elapsed = start.elapsed();

    assert!(redacted.ends_with("tail [REDACTED:github_token] end"));
    // Even unoptimized dev builds should finish well under 1 s; release finishes in ~10-20 ms.
    assert!(
        elapsed.as_secs_f64() < 1.0,
        "5 MB redaction took {elapsed:?}, expected < 1s"
    );
}
