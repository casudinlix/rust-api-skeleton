use std::path::Path;

use anyhow::{Context, bail, ensure};

use super::project::{self, FileChange};

#[derive(Debug)]
pub enum EnvironmentStatus {
    Created,
    KeyGenerated,
    Preserved,
}

/// Initialize configuration without loading application Settings or mutating the
/// process environment. Existing deployment references and keys remain intact.
pub fn initialize(root: &Path, rotate: bool, force: bool) -> anyhow::Result<EnvironmentStatus> {
    let previous = project::read_optional(root, Path::new(".env"))?;
    let source = match &previous {
        Some(contents) => contents.clone(),
        None => project::read_optional(root, Path::new(".env.example"))?
            .context(".env.example is missing; restore it before running install")?,
    };
    let parsed: Vec<_> = dotenvy::from_read_iter(source.as_bytes())
        .collect::<Result<_, _>>()
        .map_err(|_| anyhow::anyhow!("invalid .env syntax; correct it before generating a key"))?;
    let secrets: Vec<_> = parsed
        .iter()
        .filter(|(key, _)| key == "JWT_SECRET")
        .collect();
    ensure!(
        secrets.len() <= 1,
        "duplicate JWT_SECRET assignments; keep exactly one before continuing"
    );

    let lines: Vec<_> = source.split_inclusive('\n').collect();
    let matches: Vec<_> = lines
        .iter()
        .enumerate()
        .filter_map(|(index, line)| assignment(line).map(|value| (index, value)))
        .collect();
    ensure!(
        matches.len() == secrets.len(),
        "ambiguous JWT_SECRET assignment; use a single-line literal or reference"
    );
    if let Some((index, _)) = matches.first() {
        let entries: Vec<_> = dotenvy::from_read_iter(lines[*index].as_bytes())
            .collect::<Result<_, _>>()
            .map_err(|_| anyhow::anyhow!("JWT_SECRET must use a single-line literal or reference; edit multiline keys manually"))?;
        ensure!(
            entries.len() == 1 && entries[0].0 == "JWT_SECRET",
            "ambiguous JWT_SECRET assignment"
        );
    }
    let secret = secrets.first().map(|(_, value)| value.trim());
    let indirect = matches.first().is_some_and(|(_, raw)| has_reference(raw));
    let placeholder = secret.is_none_or(|value| {
        value.is_empty()
            || value.to_ascii_lowercase().contains("replace")
            || value.to_ascii_lowercase().contains("change-me")
    });
    let keep = !force && (indirect || !placeholder);
    if keep {
        if rotate {
            bail!(
                "JWT_SECRET is already configured; use --force to rotate it and invalidate existing tokens"
            );
        }
        ensure!(
            indirect || secret.is_some_and(|value| (32..=4096).contains(&value.len())),
            "JWT_SECRET is configured but invalid; correct it or explicitly run key:generate --force"
        );
        if previous.is_some() {
            return Ok(EnvironmentStatus::Preserved);
        }
    }

    let contents = if keep {
        source
    } else {
        let key = random_key()?;
        let newline = if source.contains("\r\n") {
            "\r\n"
        } else {
            "\n"
        };
        if let Some((index, _)) = matches.first() {
            let mut output = String::with_capacity(source.len() + key.len());
            for (line_index, line) in lines.iter().enumerate() {
                if line_index == *index {
                    output.push_str("JWT_SECRET=");
                    output.push_str(&key);
                    if line.ends_with('\n') {
                        output.push_str(newline);
                    }
                } else {
                    output.push_str(line);
                }
            }
            output
        } else {
            let mut output = source.clone();
            if !output.is_empty() && !output.ends_with('\n') {
                output.push_str(newline);
            }
            output.push_str("JWT_SECRET=");
            output.push_str(&key);
            output.push_str(newline);
            output
        }
    };
    let created = previous.is_none();
    project::apply_changes(
        root,
        vec![FileChange {
            relative: ".env".into(),
            before: previous,
            after: contents,
            private: true,
        }],
    )?;
    Ok(if created {
        EnvironmentStatus::Created
    } else {
        EnvironmentStatus::KeyGenerated
    })
}

fn assignment(line: &str) -> Option<&str> {
    let mut text = line.trim_start();
    if let Some(rest) = text.strip_prefix("export")
        && rest.starts_with(char::is_whitespace)
    {
        text = rest.trim_start();
    }
    let (key, value) = text.split_once('=')?;
    (key.trim() == "JWT_SECRET").then_some(value.trim())
}

/// Recognize substitution only in the value: dotenv comments, single-quoted
/// strings and escaped dollars do not refer to deployment-managed secrets.
fn has_reference(raw: &str) -> bool {
    let mut quote = None;
    let mut escaped = false;
    let mut chars = raw.chars().peekable();
    while let Some(character) = chars.next() {
        if escaped {
            escaped = false;
            continue;
        }
        if quote == Some('\'') {
            if character == '\'' {
                quote = None;
            }
            continue;
        }
        if character == '\\' {
            escaped = true;
            continue;
        }
        if character == '$'
            && chars
                .peek()
                .is_some_and(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '{')
        {
            return true;
        }
        if quote == Some('"') {
            if character == '"' {
                quote = None;
            }
            continue;
        }
        match character {
            '\'' | '"' => quote = Some(character),
            c if c.is_whitespace() => break,
            '#' if raw.starts_with('#') => break,
            _ => {}
        }
    }
    false
}

fn random_key() -> anyhow::Result<String> {
    let mut bytes = [0_u8; 32];
    getrandom::fill(&mut bytes)
        .map_err(|_| anyhow::anyhow!("OS randomness is unavailable; no JWT key was written"))?;
    let mut encoded = String::with_capacity(64);
    const HEX: &[u8; 16] = b"0123456789abcdef";
    for byte in bytes {
        encoded.push(char::from(HEX[usize::from(byte >> 4)]));
        encoded.push(char::from(HEX[usize::from(byte & 15)]));
    }
    Ok(encoded)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn fixture(contents: &str) -> tempfile::TempDir {
        let root = tempfile::tempdir().unwrap();
        fs::write(root.path().join(".env.example"), contents).unwrap();
        root
    }

    #[test]
    fn initializes_and_preserves_existing_settings_and_key() {
        let root =
            fixture("# configuration\nDB_DRIVER=mysql\nJWT_SECRET=replace-with-generated-secret\n");
        initialize(root.path(), false, false).unwrap();
        let first = fs::read_to_string(root.path().join(".env")).unwrap();
        let secret = first
            .lines()
            .find_map(|line| line.strip_prefix("JWT_SECRET="))
            .unwrap();
        assert_eq!(secret.len(), 64);
        assert!(secret.bytes().all(|b| b.is_ascii_hexdigit()));
        assert!(first.starts_with("# configuration\nDB_DRIVER=mysql\n"));
        initialize(root.path(), false, false).unwrap();
        assert_eq!(first, fs::read_to_string(root.path().join(".env")).unwrap());
    }

    #[test]
    fn key_rotation_requires_force_and_does_not_echo_existing_key() {
        let root = fixture("JWT_SECRET=replace-this-secret\n");
        initialize(root.path(), false, false).unwrap();
        let before = fs::read_to_string(root.path().join(".env")).unwrap();
        let error = initialize(root.path(), true, false).unwrap_err();
        let secret = before.trim().strip_prefix("JWT_SECRET=").unwrap();
        assert!(!format!("{error:#}").contains(secret));
        assert_eq!(
            before,
            fs::read_to_string(root.path().join(".env")).unwrap()
        );
        initialize(root.path(), true, true).unwrap();
        assert_ne!(
            before,
            fs::read_to_string(root.path().join(".env")).unwrap()
        );
    }

    #[test]
    fn rejects_duplicate_or_malformed_settings_without_writing() {
        for contents in [
            "JWT_SECRET=replace-one\nexport JWT_SECRET=replace-two\n",
            "JWT_SECRET=replace-one\nBROKEN=unquoted value\n",
            "JWT_SECRET=short-custom-key\n",
        ] {
            let root = fixture(contents);
            assert!(initialize(root.path(), false, false).is_err());
            assert!(!root.path().join(".env").exists());
        }
    }

    #[test]
    fn preserves_crlf_and_can_append_missing_secret_without_a_trailing_newline() {
        let root = fixture("DB_DRIVER=mysql\r\nAPP_ENV=local");
        initialize(root.path(), false, false).unwrap();
        let contents = fs::read_to_string(root.path().join(".env")).unwrap();
        assert!(contents.starts_with("DB_DRIVER=mysql\r\nAPP_ENV=local\r\nJWT_SECRET="));
        assert!(contents.ends_with("\r\n"));
    }

    #[test]
    fn preserves_quoted_or_external_secrets_and_unrelated_configuration() {
        for contents in [
            "# keep\nexport JWT_SECRET='a-valid-custom-secret-with-at-least-thirty-two-bytes' # keep\nAPP_ENV=local\n",
            "JWT_SECRET=${MY_DEPLOYMENT_SECRET}\nAPP_ENV=production\n",
        ] {
            let root = fixture("JWT_SECRET=replace-me\n");
            fs::write(root.path().join(".env"), contents).unwrap();
            initialize(root.path(), false, false).unwrap();
            assert_eq!(
                contents,
                fs::read_to_string(root.path().join(".env")).unwrap()
            );
        }
    }

    #[test]
    fn dollars_in_comments_and_literals_do_not_hide_missing_or_invalid_keys() {
        for contents in [
            "JWT_SECRET=replace-with-generated-secret # deploy via $JWT_SECRET\n",
            "JWT_SECRET='' # $TOKEN\n",
            "JWT_SECRET= # $TOKEN\n",
            "JWT_SECRET='replace-$TOKEN'\n",
        ] {
            let root = fixture(contents);
            initialize(root.path(), false, false).unwrap();
            let contents = fs::read_to_string(root.path().join(".env")).unwrap();
            let key = contents.trim().strip_prefix("JWT_SECRET=").unwrap();
            assert_eq!(key.len(), 64);
            assert!(key.bytes().all(|b| b.is_ascii_hexdigit()));
        }
        for contents in [
            "JWT_SECRET='literal$TOKEN'\n",
            "JWT_SECRET=literal\\$TOKEN\n",
        ] {
            let root = fixture(contents);
            assert!(initialize(root.path(), false, false).is_err());
            assert!(!root.path().join(".env").exists());
        }
    }

    #[cfg(unix)]
    #[test]
    fn private_env_and_symlink_refusal() {
        use std::os::unix::fs::{PermissionsExt, symlink};
        let root = fixture("JWT_SECRET=replace-me\n");
        initialize(root.path(), false, false).unwrap();
        assert_eq!(
            fs::metadata(root.path().join(".env"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        fs::remove_file(root.path().join(".env")).unwrap();
        let other = fixture("JWT_SECRET=untouched\n");
        symlink(other.path().join(".env.example"), root.path().join(".env")).unwrap();
        assert!(initialize(root.path(), false, false).is_err());
        assert_eq!(
            fs::read_to_string(other.path().join(".env.example")).unwrap(),
            "JWT_SECRET=untouched\n"
        );
    }
}
