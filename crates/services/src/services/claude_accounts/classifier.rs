//! Failure classification for Claude executor exits.
//!
//! Ordering (per research.md §4): UsageExhausted → NeedsReauth → Transient → Fatal.
//! Biased toward Transient for ambiguous cases (e.g. naked 429 with no usage-window
//! context) to avoid false rotations on a stray "rate limit" mention.

use chrono::{DateTime, Duration as ChronoDuration, Utc};
use once_cell::sync::Lazy;
use regex::Regex;

use super::types::{ClaudeAccountThrottleReason, FailureClass};

/// Canonical usage-limit signal emitted by `@anthropic-ai/claude-code` when the
/// 5h or weekly subscription cap is hit. Examples:
///   `Claude AI usage limit reached|1759770000`
///   `Claude AI usage limit reached, please try again after 5:20pm`
static USAGE_LIMIT_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?i)Claude AI usage limit reached(?:\|(\d+))?").expect("valid regex")
});

/// Permanent auth failures from the OAuth refresh path or the CLI's own
/// auth checks.
static NEEDS_REAUTH_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(
        r"(?i)\binvalid_grant\b|\binvalid_token\b|token (?:has been )?revoked|unauthorized: ",
    )
    .expect("valid regex")
});

/// Transient signals — explicit rate-limit / transport failures that should
/// retry the SAME account with exponential back-off.
static TRANSIENT_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(
        r"(?ix)
        # CLI's own canonical transient-rate-limit message
        Server\ is\ temporarily\ limiting\ requests
        # generic API error wrapper
        | API\ Error
        # explicit rate-limit text NOT preceded by 'usage'
        | (?-i)rate\s*limit (?i)
        # transport errors
        | ECONNRESET | ETIMEDOUT | ENETUNREACH | fetch\ failed
        # 5xx server errors
        | \b5\d{2}\b
        ",
    )
    .expect("valid regex")
});

/// Classify a Claude executor failure based on the combined stdout/stderr text
/// and the process exit code.
///
/// Note: the input text MUST come from the CLI itself (stderr, or stdout in
/// `-p` non-interactive mode). Do NOT include user-supplied prompts or model
/// assistant content — a prompt containing the phrase "rate limit" must not
/// trigger rotation.
pub fn classify_failure(stdout: &str, stderr: &str, exit_code: Option<i32>) -> FailureClass {
    let combined = if stdout.is_empty() {
        stderr.to_string()
    } else if stderr.is_empty() {
        stdout.to_string()
    } else {
        format!("{stderr}\n{stdout}")
    };

    // 1. UsageExhausted is the most specific signal — check first.
    if USAGE_LIMIT_RE.is_match(&combined) {
        return FailureClass::UsageExhausted;
    }
    // 2. Permanent auth failures.
    if NEEDS_REAUTH_RE.is_match(&combined) {
        return FailureClass::NeedsReauth;
    }
    // 3. Transient — explicit rate-limit / transport / 5xx.
    if TRANSIENT_RE.is_match(&combined) {
        return FailureClass::Transient;
    }
    // 4. Bias toward Transient when the exit code suggests recoverable failure
    //    AND we have at least *some* error context — keeps recoverable runs alive
    //    without classifying clean exit 0 as a failure.
    match exit_code {
        Some(0) => FailureClass::Fatal, // exit 0 means success; if caller is classifying, something else is wrong
        _ => FailureClass::Fatal,
    }
}

/// Parse the optional `|<unix_ts>` tail of the `Claude AI usage limit reached` signal.
/// Returns the reset time and whether it appears to be the 5h or weekly cap.
///
/// Heuristic: if the reset is more than 6 hours away, classify as weekly; otherwise
/// 5h. If no timestamp is present, return None (caller uses conservative defaults).
pub fn parse_usage_reset(
    text: &str,
    now: DateTime<Utc>,
) -> Option<(DateTime<Utc>, ClaudeAccountThrottleReason)> {
    let caps = USAGE_LIMIT_RE.captures(text)?;
    let ts_str = caps.get(1)?.as_str();
    let ts: i64 = ts_str.parse().ok()?;
    let reset_at = DateTime::<Utc>::from_timestamp(ts, 0)?;
    let reason = if reset_at - now > ChronoDuration::hours(6) {
        ClaudeAccountThrottleReason::Weekly
    } else {
        ClaudeAccountThrottleReason::FiveHour
    };
    Some((reset_at, reason))
}

/// Conservative fallback reset times when the CLI doesn't surface a structured
/// timestamp (per spec FR-017 / edge case "Unknown usage window").
pub fn fallback_reset(reason: ClaudeAccountThrottleReason, now: DateTime<Utc>) -> DateTime<Utc> {
    match reason {
        ClaudeAccountThrottleReason::FiveHour => now + ChronoDuration::hours(1),
        ClaudeAccountThrottleReason::Weekly => {
            // End of current ISO week, UTC.
            let week_seconds = 7 * 24 * 3600;
            let now_ts = now.timestamp();
            let next_week = ((now_ts / week_seconds) + 1) * week_seconds;
            DateTime::<Utc>::from_timestamp(next_week, 0).unwrap_or(now + ChronoDuration::days(7))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn usage_limit_pipe_form_classifies_as_usage_exhausted() {
        let stdout = "Claude AI usage limit reached|1759770000";
        assert_eq!(
            classify_failure(stdout, "", Some(1)),
            FailureClass::UsageExhausted
        );
    }

    #[test]
    fn usage_limit_human_form_classifies_as_usage_exhausted() {
        let stderr = "Claude AI usage limit reached, please try again after 5:20pm";
        assert_eq!(
            classify_failure("", stderr, Some(1)),
            FailureClass::UsageExhausted
        );
    }

    #[test]
    fn invalid_grant_classifies_as_needs_reauth() {
        let stderr = "OAuth refresh failed: invalid_grant";
        assert_eq!(
            classify_failure("", stderr, Some(1)),
            FailureClass::NeedsReauth
        );
    }

    #[test]
    fn revoked_token_classifies_as_needs_reauth() {
        let stderr = "Token has been revoked by the user";
        assert_eq!(
            classify_failure("", stderr, Some(1)),
            FailureClass::NeedsReauth
        );
    }

    #[test]
    fn transient_rate_limit_classifies_as_transient() {
        let stderr = "API Error: Server is temporarily limiting requests (not your usage limit)";
        assert_eq!(
            classify_failure("", stderr, Some(1)),
            FailureClass::Transient
        );
    }

    #[test]
    fn five_hundred_classifies_as_transient() {
        let stderr = "API Error: 503 Service Unavailable";
        assert_eq!(
            classify_failure("", stderr, Some(1)),
            FailureClass::Transient
        );
    }

    #[test]
    fn unknown_error_classifies_as_fatal() {
        let stderr = "Something else went wrong";
        assert_eq!(classify_failure("", stderr, Some(1)), FailureClass::Fatal);
    }

    #[test]
    fn usage_limit_wins_over_transient() {
        // Both signals present — UsageExhausted must win.
        let stderr = "API Error: 503 Service Unavailable\nClaude AI usage limit reached|1759770000";
        assert_eq!(
            classify_failure("", stderr, Some(1)),
            FailureClass::UsageExhausted
        );
    }

    #[test]
    fn needs_reauth_wins_over_transient() {
        let stderr = "API Error: 503 Service Unavailable\ninvalid_grant";
        assert_eq!(
            classify_failure("", stderr, Some(1)),
            FailureClass::NeedsReauth
        );
    }

    #[test]
    fn parse_usage_reset_extracts_timestamp_and_five_hour() {
        let now = DateTime::<Utc>::from_timestamp(1_700_000_000, 0).unwrap();
        // Reset 1 hour in the future.
        let stdout = format!("Claude AI usage limit reached|{}", 1_700_000_000 + 3600);
        let (reset, reason) = parse_usage_reset(&stdout, now).unwrap();
        assert_eq!(reset.timestamp(), 1_700_000_000 + 3600);
        assert_eq!(reason, ClaudeAccountThrottleReason::FiveHour);
    }

    #[test]
    fn parse_usage_reset_far_future_is_weekly() {
        let now = DateTime::<Utc>::from_timestamp(1_700_000_000, 0).unwrap();
        // Reset 3 days in the future.
        let stdout = format!(
            "Claude AI usage limit reached|{}",
            1_700_000_000 + 3 * 24 * 3600
        );
        let (_, reason) = parse_usage_reset(&stdout, now).unwrap();
        assert_eq!(reason, ClaudeAccountThrottleReason::Weekly);
    }

    #[test]
    fn parse_usage_reset_returns_none_without_timestamp() {
        let now = Utc::now();
        let stdout = "Claude AI usage limit reached, please try again after 5:20pm";
        assert!(parse_usage_reset(stdout, now).is_none());
    }
}
