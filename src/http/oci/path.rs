//! Repository names contain `/`, and axum matches a wildcard only at the end of a
//! path, so `/v2/{*name}/blobs/{digest}` cannot be a route. Before routing, the
//! name is folded into one segment with `%2F`:
//!
//! ```text
//! /team/app/blobs/uploads/abc  ->  /team%2Fapp/blobs/uploads/abc
//! ```
//!
//! after which plain routes like `/{repo}/blobs/uploads/{id}` match and `Path`
//! decodes the name back. Paths are relative to `/v2`. A path that does not fit, or
//! whose name is not a valid repository name, is left alone and ends in a 404.

use crate::oci::RepoName;
use axum::extract::Request;
use axum::http::Uri;
use axum::http::uri::PathAndQuery;

/// What follows the repository name in every OCI path, longest first so that
/// `/blobs/uploads/` wins over `/blobs/` at the same position.
const MARKERS: [&str; 5] = [
    "/blobs/uploads/",
    "/blobs/",
    "/manifests/",
    "/tags/list",
    "/referrers/",
];

pub fn fold_repo_name(mut req: Request) -> Request {
    if let Some(uri) = folded(req.uri()) {
        *req.uri_mut() = uri;
    }
    req
}

fn folded(uri: &Uri) -> Option<Uri> {
    let path = fold_path(uri.path())?;
    let path_and_query = match uri.query() {
        Some(q) => format!("{path}?{q}"),
        None => path,
    };

    let mut parts = uri.clone().into_parts();
    parts.path_and_query = Some(path_and_query.parse::<PathAndQuery>().ok()?);
    Uri::from_parts(parts).ok()
}

/// The marker that starts furthest right decides where the name ends: the name is
/// what precedes the last OCI suffix in the path.
fn fold_path(path: &str) -> Option<String> {
    let rest = path.strip_prefix('/')?;
    let (start, marker) = MARKERS
        .iter()
        .filter_map(|m| rest.rfind(m).map(|i| (i, *m)))
        .max_by_key(|(i, m)| (*i, m.len()))?;

    let name = &rest[..start];
    if !name.contains('/') {
        return None;
    }
    name.parse::<RepoName>().ok()?;
    Some(format!(
        "/{}{marker}{}",
        name.replace('/', "%2F"),
        &rest[start + marker.len()..]
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn folds_multi_segment_names() {
        let cases = [
            (
                "/team/app/blobs/uploads/",
                Some("/team%2Fapp/blobs/uploads/"),
            ),
            (
                "/team/app/blobs/uploads/abc",
                Some("/team%2Fapp/blobs/uploads/abc"),
            ),
            ("/a/b/c/blobs/sha256:00", Some("/a%2Fb%2Fc/blobs/sha256:00")),
            ("/a/b/manifests/v1", Some("/a%2Fb/manifests/v1")),
            ("/a/b/tags/list", Some("/a%2Fb/tags/list")),
            (
                "/a/b/referrers/sha256:00",
                Some("/a%2Fb/referrers/sha256:00"),
            ),
            // A repository may be called `blobs/uploads`: the last marker counts.
            (
                "/x/blobs/uploads/blobs/sha256:00",
                Some("/x%2Fblobs%2Fuploads/blobs/sha256:00"),
            ),
            // One segment needs no folding; plain routes match it.
            ("/app/blobs/uploads/", None),
            ("/", None),
            ("/a/b/unknown", None),
            // Not a repository name: left for the 404.
            ("/Team/App/blobs/uploads/", None),
            ("/a//b/blobs/uploads/", None),
        ];
        for (path, expected) in cases {
            assert_eq!(fold_path(path).as_deref(), expected, "{path}");
        }
    }

    #[test]
    fn keeps_query() {
        let uri: Uri = "/team/app/blobs/uploads/?mount=sha256:00&from=team/base"
            .parse()
            .unwrap_or_else(|e| panic!("{e}"));
        let folded = folded(&uri).map(|u| u.to_string());
        assert_eq!(
            folded.as_deref(),
            Some("/team%2Fapp/blobs/uploads/?mount=sha256:00&from=team/base")
        );
    }
}
