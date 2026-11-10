//! Classification shared by completion gates; a token is not proof of impact.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct FlagClaims {
    pub placeholder: bool,
    pub concrete: bool,
}

pub(crate) fn flag_claims(text: &str) -> FlagClaims {
    let lower = text.to_lowercase();
    let mut claims = FlagClaims::default();
    for (index, _) in lower.match_indices("flag{") {
        let body = &lower[index + 5..];
        let Some(close) = body.find('}') else {
            continue;
        };
        let core: String = body[..close]
            .chars()
            .filter(|c| !matches!(c, '\\' | '_'))
            .collect();
        let placeholder = core.is_empty()
            || core
                .chars()
                .all(|c| matches!(c, '.' | '…' | '+' | '-' | ' ' | '*' | 'x' | 'a'))
            || core == "test"
            || (core.len() >= 3
                && core
                    .chars()
                    .next()
                    .is_some_and(|first| core.chars().all(|c| c == first)));
        claims.placeholder |= placeholder;
        claims.concrete |= !placeholder;
    }
    claims
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn classifies_tokens_after_the_matched_prefix_in_unicode_text() {
        for text in [
            "flag{...}",
            "flag{test}",
            "Flag{AAAA}",
            "记录：flag{_..._}",
            "flag{}",
        ] {
            assert_eq!(
                flag_claims(text),
                FlagClaims {
                    placeholder: true,
                    concrete: false
                },
                "{text}"
            );
        }
        assert_eq!(
            flag_claims("证据 flag{value_123}"),
            FlagClaims {
                placeholder: false,
                concrete: true
            }
        );
        assert_eq!(
            flag_claims("flag{...} then flag{value_123}"),
            FlagClaims {
                placeholder: true,
                concrete: true
            }
        );
        assert_eq!(
            flag_claims("flag, incomplete flag{value"),
            FlagClaims::default()
        );
    }
}
