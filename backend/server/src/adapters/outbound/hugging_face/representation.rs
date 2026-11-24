use std::fmt::Write as _;

use hyper::Uri;
use sha2::{Digest, Sha256};

pub(super) fn fingerprint(uri: &Uri) -> String {
    let value = cdn_resource(uri).unwrap_or_else(|| uri.to_string());
    hash(&value)
}

pub(super) fn matches(recorded: &str, uri: &Uri) -> bool {
    fingerprint(uri) == recorded
        // Older markers used the full URI. Preserve their exact-match behavior.
        || hash(&uri.to_string()) == recorded
}

fn hash(value: &str) -> String {
    let digest = Sha256::digest(value.as_bytes());
    let mut out = String::with_capacity(64);
    for byte in digest {
        let _ = write!(out, "{byte:02x}");
    }
    out
}

fn cdn_resource(uri: &Uri) -> Option<String> {
    let authority = uri.authority()?.as_str();
    let path = uri.path();
    if uri.scheme_str() != Some("https")
        || authority != "us.aws.cdn.hf.co"
        || !path.starts_with("/xet-bridge-us/")
    {
        return None;
    }

    // The CDN rotates the authorization and reorders unique parameter names.
    // Only the marker digest uses this identity. The request uses the signed URI.
    let mut auth = [false; 4];
    let mut resource_query = Vec::new();
    for parameter in uri.query()?.split('&') {
        let (key, value) = parameter.split_once('=')?;
        let index = match key {
            "Expires" => 0,
            "Policy" => 1,
            "Signature" => 2,
            "Key-Pair-Id" => 3,
            _ => {
                if !plain_name(key) {
                    return None;
                }
                resource_query.push((key, parameter));
                continue;
            }
        };
        if auth[index] || value.is_empty() || (index == 0 && value.parse::<u64>().is_err()) {
            return None;
        }
        auth[index] = true;
        if index == 3 {
            resource_query.push((key, parameter));
        }
    }
    resource_query.sort_unstable_by_key(|(key, _)| *key);
    if !auth.into_iter().all(|field| field)
        || resource_query.windows(2).any(|pair| pair[0].0 == pair[1].0)
    {
        return None;
    }
    let query = resource_query
        .into_iter()
        .map(|(_, parameter)| parameter)
        .collect::<Vec<_>>()
        .join("&");
    // Keep normalized identities disjoint from hashes of complete URIs.
    Some(format!("hf-xet-cdn-v1\0https://{authority}{path}?{query}"))
}

fn plain_name(key: &str) -> bool {
    !key.is_empty()
        && key
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signature_rotation_preserves_the_exact_cdn_resource()
    -> Result<(), hyper::http::uri::InvalidUri> {
        let first: Uri = "https://us.aws.cdn.hf.co/xet-bridge-us/repo/blob?Hash-Algorithm=SHA256&Expires=10&Policy=policyA&Signature=alpha&Key-Pair-Id=key&user_id=anon".parse()?;
        let rotated: Uri = "https://us.aws.cdn.hf.co/xet-bridge-us/repo/blob?user_id=anon&Hash-Algorithm=SHA256&Expires=20&Policy=policyB&Signature=beta&Key-Pair-Id=key".parse()?;
        assert_eq!(fingerprint(&first), fingerprint(&rotated));
        let legacy = hash(&first.to_string());
        assert!(matches(&legacy, &first));
        assert!(!matches(&legacy, &rotated));
        for changed in [
            first.to_string().replace("user_id=anon", "user_id=other"),
            first.to_string().replace("SHA256", "Other"),
            first.to_string().replace("repo/blob", "repo/other"),
            first
                .to_string()
                .replace("Signature=alpha", "Signature=alpha&Signature=beta"),
            first.to_string().replace("Policy=policyA", "Policy="),
            first.to_string().replace("Expires=10", "Expires=soon"),
            first
                .to_string()
                .replace("Key-Pair-Id=key", "Key-Pair-Id=other"),
        ] {
            assert_ne!(fingerprint(&first), fingerprint(&changed.parse()?));
        }
        Ok(())
    }

    #[test]
    fn duplicate_or_encoded_query_names_keep_full_uri_identity()
    -> Result<(), hyper::http::uri::InvalidUri> {
        for query in ["user_id=anon&user_id=other", "user%5Fid=anon", "=anon"] {
            let uri: Uri = format!(
                "https://us.aws.cdn.hf.co/xet-bridge-us/repo/blob?Expires=10&Policy=p&Signature=a&Key-Pair-Id=k&{query}"
            )
            .parse()?;
            assert_eq!(fingerprint(&uri), hash(&uri.to_string()));
        }
        Ok(())
    }

    #[test]
    fn signed_and_raw_resource_markers_use_separate_hash_inputs()
    -> Result<(), hyper::http::uri::InvalidUri> {
        let raw: Uri =
            "https://us.aws.cdn.hf.co/xet-bridge-us/repo/blob?Key-Pair-Id=key&user_id=anon"
                .parse()?;
        let signed: Uri = format!("{raw}&Expires=10&Policy=policy&Signature=alpha").parse()?;
        assert!(!matches(&fingerprint(&signed), &raw));
        assert!(!matches(&fingerprint(&raw), &signed));
        Ok(())
    }

    #[test]
    fn other_endpoints_keep_full_uri_identity() -> Result<(), hyper::http::uri::InvalidUri> {
        for prefix in [
            "http://us.aws.cdn.hf.co/xet-bridge-us/repo/blob",
            "https://us.aws.cdn.hf.co:8443/xet-bridge-us/repo/blob",
            "https://us.aws.cdn.hf.co/other/repo/blob",
            "https://cdn-lfs.hf.co/xet-bridge-us/repo/blob",
            "http://127.0.0.1:8080/xet-bridge-us/repo/blob",
        ] {
            let first: Uri =
                format!("{prefix}?Expires=10&Policy=p&Signature=a&Key-Pair-Id=k").parse()?;
            let next: Uri =
                format!("{prefix}?Expires=20&Policy=p&Signature=b&Key-Pair-Id=k").parse()?;
            assert_ne!(fingerprint(&first), fingerprint(&next));
        }
        Ok(())
    }
}
