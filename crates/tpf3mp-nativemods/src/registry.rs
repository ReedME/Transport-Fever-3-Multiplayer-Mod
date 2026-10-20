//! A server's index of native mods, made from a registry: a git repository
//! that lists the mods' own repositories, as CKAN's metadata repository
//! lists Kerbal mods (proposed D29 amendment, docs/NATIVE_MODS.md, "A
//! server's own index").
//!
//! The registry is one file, [`LIST_FILE`], in its repository: one address
//! of a mod's repository per line, `#` starting a comment. Each mod's
//! repository holds [`MOD_FILE`] at its top: its packages in the index's
//! format ([`ModFile`]), whose names and descriptions are what players see
//! of it, its preview. The server reads the list and every mod's file again
//! every so often and serves the index made of them to its players.
//!
//! Each mod stands on its own: a mod whose file cannot be read, or holds a
//! malformed package, or claims an id an earlier mod in the list has, is
//! left out with the reason, and the others are served. A package's
//! `homepage` is its repository unless its file names another, its
//! `authors` the repository's owner unless it names them, and its
//! `preview` image may be a path in its repository.
//!
//! **Listings.** For a mod on GitHub, the server also asks GitHub how often
//! its releases were downloaded and when it last changed ([`listings`]):
//! shown to players beside the mod, never used to choose what is
//! installed.
//!
//! Files are read over HTTPS from the repository's host: GitHub's and
//! GitLab's raw files of the default branch ([`raw_url`]), or an address of
//! the file itself.
//!
//! **Proposed mods.** A mod is added by a pull request to the registry that
//! adds its repository to the list. While such a request is open, the mods
//! it would add are [`Proposals`]: served apart from the index, each with
//! its pull request, for players who ask to see them. Nobody has reviewed
//! them; a player installs one only by choosing it, and a room never gets
//! one by itself. Only GitHub registries have them ([`pulls_url`]).

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::index::{FORMAT, Index, Listing, MAX_INDEX, Package};

/// The registry's list of mods, at the top of its repository.
pub const LIST_FILE: &str = "mods.txt";
/// A mod's file, at the top of its repository.
pub const MOD_FILE: &str = "tpf3mp-native.json";
/// Most mods a registry lists.
pub const MAX_MODS: usize = 256;
/// Largest list, and largest mod file.
pub const MAX_LIST: u64 = 64 * 1024;
pub const MAX_MOD_FILE: u64 = 256 * 1024;
/// Longest description a package shows players.
pub const MAX_DESCRIPTION: usize = 500;

/// A mod's file: its packages, each a version of one of its mods.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModFile {
    pub format: u32,
    pub packages: Vec<Package>,
}

/// The repositories `list` names, in order, each once: `https` addresses
/// only, at most [`MAX_MODS`].
pub fn parse_list(list: &str) -> (Vec<String>, Vec<String>) {
    let mut repositories = Vec::new();
    let mut problems = Vec::new();
    let mut seen = BTreeSet::new();
    for (number, line) in list.lines().enumerate() {
        let line = line.split('#').next().unwrap_or_default().trim();
        if line.is_empty() {
            continue;
        }
        let address = line.trim_end_matches('/').trim_end_matches(".git");
        if !address.starts_with("https://") || address.contains(char::is_whitespace) {
            problems.push(format!("line {}: not an https address", number + 1));
        } else if repositories.len() == MAX_MODS {
            problems.push(format!("more than {MAX_MODS} mods; the rest are left out"));
            break;
        } else if seen.insert(address.to_ascii_lowercase()) {
            repositories.push(address.to_owned());
        }
    }
    (repositories, problems)
}

/// Where the file `path` at the top of `repository`'s default branch is
/// read from: GitHub's or GitLab's raw files; an address ending in `.json`
/// or `.txt` is the file itself.
pub fn raw_url(repository: &str, path: &str) -> Option<String> {
    let repository = repository.trim_end_matches('/');
    if repository.ends_with(".json") || repository.ends_with(".txt") {
        return Some(repository.to_owned());
    }
    let parts = |host: &str| -> Option<(String, String)> {
        let rest = repository.strip_prefix(host)?;
        let mut parts = rest.split('/');
        let owner = parts.next().filter(|p| !p.is_empty())?;
        let name = parts.next().filter(|p| !p.is_empty())?;
        parts
            .next()
            .is_none()
            .then(|| (owner.to_owned(), name.to_owned()))
    };
    if let Some((owner, name)) = parts("https://github.com/") {
        return Some(format!(
            "https://raw.githubusercontent.com/{owner}/{name}/HEAD/{path}"
        ));
    }
    if let Some(rest) = repository.strip_prefix("https://gitlab.com/")
        && rest.split('/').all(|part| !part.is_empty())
        && rest.contains('/')
    {
        return Some(format!("https://gitlab.com/{rest}/-/raw/HEAD/{path}"));
    }
    None
}

/// The index a server serves, and what was left out of it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Built {
    pub index: Index,
    /// The index as served: its exact JSON.
    pub json: Vec<u8>,
    /// Each mod left out, and why.
    pub problems: Vec<String>,
    /// The repository each mod's id came from.
    pub sources: BTreeMap<String, String>,
}

impl Built {
    /// Adds what the mods' hosts say of them to the index, unless that
    /// makes it too large to serve.
    pub fn add_listings(&mut self, listings: BTreeMap<String, Listing>) {
        let mut index = self.index.clone();
        index.listings = listings;
        if let Ok(json) = serde_json::to_vec(&index)
            && json.len() as u64 <= MAX_INDEX
        {
            self.index = index;
            self.json = json;
        } else {
            self.problems
                .push("the mods' downloads and dates are left out: the index is too large".into());
        }
    }
}

/// The index of the mods `list` names, each mod's file read with `fetch`
/// (an address, at most a limit of bytes), numbered `serial`.
pub fn build(
    list: &str,
    serial: u64,
    fetch: impl Fn(&str, u64) -> Result<Vec<u8>, String>,
) -> Built {
    let (repositories, mut problems) = parse_list(list);
    let mut packages: Vec<Package> = Vec::new();
    // Which repository each id came from: the first in the list keeps it.
    let mut owners: BTreeMap<String, String> = BTreeMap::new();
    for repository in &repositories {
        match read_mod(repository, &fetch) {
            Ok(mine) => {
                if let Some(taken) = mine.iter().find_map(|p| {
                    owners
                        .get(&p.id)
                        .filter(|owner| *owner != repository)
                        .map(|owner| (p.id.clone(), owner.clone()))
                }) {
                    problems.push(format!("{repository}: the id {} is {}'s", taken.0, taken.1));
                    continue;
                }
                for package in mine {
                    owners.insert(package.id.clone(), repository.clone());
                    packages.push(package);
                }
            }
            Err(problem) => problems.push(format!("{repository}: {problem}")),
        }
    }
    let mut index = Index {
        format: FORMAT,
        serial,
        packages,
        listings: Default::default(),
    };
    let mut json = serde_json::to_vec(&index).unwrap_or_default();
    // Too large to serve: the last mods go first.
    while json.len() as u64 > MAX_INDEX && !index.packages.is_empty() {
        let dropped = index.packages.pop().map(|p| p.id).unwrap_or_default();
        index.packages.retain(|p| p.id != dropped);
        problems.push(format!("{dropped}: the index is too large with it"));
        json = serde_json::to_vec(&index).unwrap_or_default();
    }
    owners.retain(|id, _| index.packages.iter().any(|p| &p.id == id));
    Built {
        index,
        json,
        problems,
        sources: owners,
    }
}

/// The packages of the mod in `repository`, each checked as the index
/// checks them.
fn read_mod(
    repository: &str,
    fetch: &impl Fn(&str, u64) -> Result<Vec<u8>, String>,
) -> Result<Vec<Package>, String> {
    let url = raw_url(repository, MOD_FILE)
        .ok_or("not a GitHub or GitLab repository, nor the address of its file")?;
    let bytes = fetch(&url, MAX_MOD_FILE)?;
    let file: ModFile =
        serde_json::from_slice(&bytes).map_err(|error| format!("{MOD_FILE}: {error}"))?;
    if file.format != FORMAT {
        return Err(format!("{MOD_FILE} has format {}", file.format));
    }
    let homepage = (!repository.ends_with(".json")).then(|| repository.to_owned());
    let owner = github_repo(repository).map(|(owner, _)| owner);
    let mut packages = file.packages;
    for package in &mut packages {
        if package.authors.is_empty()
            && let Some(owner) = &owner
        {
            package.authors.push(owner.clone());
        }
        if let Some(preview) = package.preview.take() {
            package.preview = Some(if preview.starts_with("https://") {
                preview
            } else if preview.split('/').all(|part| {
                !part.is_empty()
                    && part != "."
                    && part != ".."
                    && part
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
            }) {
                raw_url(repository, &preview)
                    .filter(|_| !repository.ends_with(".json"))
                    .ok_or_else(|| {
                        format!(
                            "{} {}: the preview {preview:?} is a path, but the mod is not in a repository",
                            package.id, package.version
                        )
                    })?
            } else {
                return Err(format!(
                    "{} {}: the preview {preview:?} is neither an https address nor a plain path",
                    package.id, package.version
                ));
            });
        }
        if package.description.len() > MAX_DESCRIPTION {
            return Err(format!(
                "{} {}: the description is longer than {MAX_DESCRIPTION} bytes",
                package.id, package.version
            ));
        }
        if package.homepage.is_none() {
            package.homepage.clone_from(&homepage);
        }
    }
    // The index's own checks, this mod's packages alone.
    let alone = Index {
        format: FORMAT,
        serial: 0,
        packages,
        listings: Default::default(),
    };
    alone.validate()?;
    if alone.packages.is_empty() {
        return Err(format!("{MOD_FILE} lists no package"));
    }
    Ok(alone.packages)
}

/// The owner and name of a GitHub repository's address.
fn github_repo(repository: &str) -> Option<(String, String)> {
    let rest = repository
        .trim_end_matches('/')
        .strip_prefix("https://github.com/")?;
    let mut parts = rest.split('/');
    let owner = parts.next().filter(|p| !p.is_empty())?;
    let name = parts.next().filter(|p| !p.is_empty())?;
    let safe = |text: &str| {
        text.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
    };
    (parts.next().is_none() && safe(owner) && safe(name))
        .then(|| (owner.to_owned(), name.to_owned()))
}

/// What GitHub says of each mod in `sources` (id to repository): its
/// releases' downloads and when it last changed. `api` reads an address of
/// GitHub's API. A repository GitHub does not answer for is left out, with
/// the reason; mods not on GitHub have none.
pub fn listings(
    sources: &BTreeMap<String, String>,
    api: impl Fn(&str) -> Result<Vec<u8>, String>,
) -> (BTreeMap<String, Listing>, Vec<String>) {
    #[derive(Deserialize)]
    struct Asset {
        download_count: u64,
    }
    #[derive(Deserialize)]
    struct Release {
        published_at: Option<String>,
        #[serde(default)]
        assets: Vec<Asset>,
    }
    #[derive(Deserialize)]
    struct Repo {
        pushed_at: Option<String>,
    }
    let day = |stamp: &str| {
        stamp
            .get(..10)
            .filter(|day| crate::index::is_date(day))
            .map(str::to_owned)
    };
    let mut by_repository: BTreeMap<&str, Option<Listing>> = BTreeMap::new();
    let mut problems = Vec::new();
    for repository in sources.values() {
        if by_repository.contains_key(repository.as_str()) {
            continue;
        }
        let Some((owner, name)) = github_repo(repository) else {
            by_repository.insert(repository, None);
            continue;
        };
        let base = format!("https://api.github.com/repos/{owner}/{name}");
        let read = || -> Result<Listing, String> {
            let releases: Vec<Release> =
                serde_json::from_slice(&api(&format!("{base}/releases?per_page=100"))?)
                    .map_err(|e| e.to_string())?;
            if releases.is_empty() {
                let repo: Repo = serde_json::from_slice(&api(&base)?).map_err(|e| e.to_string())?;
                return Ok(Listing {
                    downloads: None,
                    updated: repo.pushed_at.as_deref().and_then(day),
                });
            }
            Ok(Listing {
                downloads: Some(
                    releases
                        .iter()
                        .flat_map(|r| &r.assets)
                        .map(|a| a.download_count)
                        .sum(),
                ),
                updated: releases
                    .iter()
                    .filter_map(|r| r.published_at.as_deref().and_then(day))
                    .max(),
            })
        };
        match read() {
            Ok(listing) => {
                by_repository.insert(repository, Some(listing));
            }
            Err(problem) => {
                problems.push(format!("{repository}: GitHub: {problem}"));
                by_repository.insert(repository, None);
            }
        }
    }
    let listings = sources
        .iter()
        .filter_map(|(id, repository)| {
            by_repository
                .get(repository.as_str())
                .cloned()
                .flatten()
                .map(|listing| (id.clone(), listing))
        })
        .collect();
    (listings, problems)
}

/// An open pull request to the registry, with the list it proposes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pull {
    pub number: u64,
    pub title: String,
    /// Its page.
    pub url: String,
    /// Where its list is read from: the list file at its head commit.
    pub list_url: String,
}

/// The pull request that proposes a mod, as players see it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Proposal {
    pub number: u64,
    pub title: String,
    pub url: String,
}

/// The mods open pull requests to the registry would add: not reviewed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Proposals {
    pub format: u32,
    pub serial: u64,
    pub packages: Vec<Package>,
    /// The pull request proposing each package, by its id.
    pub pulls: BTreeMap<String, Proposal>,
}

impl Proposals {
    /// Them as an index to install from.
    pub fn index(&self) -> Index {
        Index {
            format: self.format,
            serial: self.serial,
            packages: self.packages.clone(),
            listings: Default::default(),
        }
    }

    /// The proposals in `json`, if every package checks out as an index's
    /// and each has its pull request.
    pub fn parse(json: &[u8]) -> Result<Self, String> {
        let proposals: Self = serde_json::from_slice(json).map_err(|e| e.to_string())?;
        if proposals.format != FORMAT {
            return Err(format!("format {}", proposals.format));
        }
        proposals.index().validate()?;
        if let Some(p) = proposals
            .packages
            .iter()
            .find(|p| !proposals.pulls.contains_key(&p.id))
        {
            return Err(format!("{} names no pull request", p.id));
        }
        Ok(proposals)
    }
}

/// Where a GitHub registry's open pull requests are listed (its API), or
/// none for a registry elsewhere.
pub fn pulls_url(registry: &str) -> Option<String> {
    let rest = registry
        .trim_end_matches('/')
        .trim_end_matches(".git")
        .strip_prefix("https://github.com/")?;
    let (owner, name) = rest.split_once('/')?;
    (!owner.is_empty() && !name.is_empty() && !name.contains('/')).then(|| {
        format!("https://api.github.com/repos/{owner}/{name}/pulls?state=open&per_page=50")
    })
}

/// The open pull requests in GitHub's answer `json`: those whose head is
/// still there, in GitHub's order.
pub fn parse_pulls(json: &[u8]) -> Result<Vec<Pull>, String> {
    #[derive(Deserialize)]
    struct Repo {
        full_name: String,
    }
    #[derive(Deserialize)]
    struct Head {
        sha: String,
        repo: Option<Repo>,
    }
    #[derive(Deserialize)]
    struct Raw {
        number: u64,
        title: String,
        html_url: String,
        head: Head,
    }
    let raw: Vec<Raw> = serde_json::from_slice(json).map_err(|e| e.to_string())?;
    let safe = |text: &str| {
        !text.is_empty()
            && text
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '/'))
    };
    Ok(raw
        .into_iter()
        .filter_map(|pull| {
            let repo = pull.head.repo?;
            (safe(&repo.full_name) && pull.head.sha.chars().all(|c| c.is_ascii_hexdigit())).then(
                || Pull {
                    number: pull.number,
                    title: pull.title.chars().take(120).collect(),
                    url: pull.html_url,
                    list_url: format!(
                        "https://raw.githubusercontent.com/{}/{}/{LIST_FILE}",
                        repo.full_name, pull.head.sha
                    ),
                },
            )
        })
        .collect())
}

/// The mods `pulls` would add to the registry's `list`: the repositories a
/// pull request's list has and `list` does not, each read as [`build`]
/// reads them. A mod whose id the `accepted` index or an earlier proposal
/// has is left out.
pub fn propose(
    list: &str,
    accepted: &Index,
    pulls: &[Pull],
    serial: u64,
    fetch: impl Fn(&str, u64) -> Result<Vec<u8>, String>,
) -> (Proposals, Vec<u8>, Vec<String>) {
    let (listed, _) = parse_list(list);
    let listed: BTreeSet<String> = listed.iter().map(|r| r.to_ascii_lowercase()).collect();
    let mut proposals = Proposals {
        format: FORMAT,
        serial,
        packages: Vec::new(),
        pulls: BTreeMap::new(),
    };
    let mut problems = Vec::new();
    let mut read = BTreeSet::new();
    for pull in pulls {
        let theirs = match fetch(&pull.list_url, MAX_LIST)
            .and_then(|bytes| String::from_utf8(bytes).map_err(|e| e.to_string()))
        {
            Ok(text) => parse_list(&text).0,
            Err(problem) => {
                problems.push(format!("pull request #{}: {problem}", pull.number));
                continue;
            }
        };
        for repository in theirs {
            let key = repository.to_ascii_lowercase();
            if listed.contains(&key) || !read.insert(key) {
                continue;
            }
            match read_mod(&repository, &fetch) {
                Ok(mine) => {
                    if let Some(taken) = mine.iter().find(|p| {
                        !accepted.versions(&p.id).is_empty() || proposals.pulls.contains_key(&p.id)
                    }) {
                        problems.push(format!(
                            "pull request #{}: {repository}: the id {} is taken",
                            pull.number, taken.id
                        ));
                        continue;
                    }
                    for package in mine {
                        proposals.pulls.insert(
                            package.id.clone(),
                            Proposal {
                                number: pull.number,
                                title: pull.title.clone(),
                                url: pull.url.clone(),
                            },
                        );
                        proposals.packages.push(package);
                    }
                }
                Err(problem) => problems.push(format!(
                    "pull request #{}: {repository}: {problem}",
                    pull.number
                )),
            }
        }
    }
    let mut json = serde_json::to_vec(&proposals).unwrap_or_default();
    while json.len() as u64 > MAX_INDEX && !proposals.packages.is_empty() {
        let dropped = proposals.packages.pop().map(|p| p.id).unwrap_or_default();
        proposals.packages.retain(|p| p.id != dropped);
        proposals.pulls.remove(&dropped);
        problems.push(format!("{dropped}: the proposals are too large with it"));
        json = serde_json::to_vec(&proposals).unwrap_or_default();
    }
    (proposals, json, problems)
}

/// The serial of an index built now, after `previous`: the time in
/// seconds, so that it grows across restarts too, and always past the one
/// before.
pub fn next_serial(previous: u64, now_seconds: u64) -> u64 {
    now_seconds.max(previous.saturating_add(1))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::index::tests::{BUILD, package};

    fn file(packages: Vec<Package>) -> Vec<u8> {
        serde_json::to_vec(&ModFile {
            format: FORMAT,
            packages,
        })
        .unwrap()
    }

    #[test]
    fn a_list_names_https_repositories_once() {
        let (repositories, problems) = parse_list(
            "# the mods\nhttps://github.com/a/bigmap\n\nhttps://github.com/a/bigmap.git/ # again\n\
             http://example.com/plain\nhttps://gitlab.com/b/trees\n",
        );
        assert_eq!(
            repositories,
            ["https://github.com/a/bigmap", "https://gitlab.com/b/trees"]
        );
        assert_eq!(problems, ["line 5: not an https address"]);
    }

    #[test]
    fn a_mod_file_is_read_from_its_repositorys_host() {
        assert_eq!(
            raw_url("https://github.com/a/bigmap", MOD_FILE).unwrap(),
            "https://raw.githubusercontent.com/a/bigmap/HEAD/tpf3mp-native.json"
        );
        assert_eq!(
            raw_url("https://gitlab.com/group/sub/trees", MOD_FILE).unwrap(),
            "https://gitlab.com/group/sub/trees/-/raw/HEAD/tpf3mp-native.json"
        );
        assert_eq!(
            raw_url("https://example.com/mods/x.json", MOD_FILE).unwrap(),
            "https://example.com/mods/x.json"
        );
        assert_eq!(raw_url("https://github.com/a", MOD_FILE), None);
        assert_eq!(raw_url("https://example.com/a/b", MOD_FILE), None);
    }

    #[test]
    fn each_mod_stands_on_its_own() {
        let mut big = package("bigmap", "0.3.0", &[("data.txt", b"x")]);
        big.description = "Maps up to 128 km a side.".into();
        let thief = package("bigmap", "9.0.0", &[]);
        let mut broken = package("trees", "1.0.0", &[]);
        broken.builds.clear();
        let mut own_page = package("signals", "1.0.0", &[]);
        own_page.homepage = Some("https://example.com/signals".into());
        let files: BTreeMap<&str, Vec<u8>> = [
            (
                "https://raw.githubusercontent.com/a/bigmap/HEAD/tpf3mp-native.json",
                file(vec![big.clone()]),
            ),
            (
                "https://raw.githubusercontent.com/z/copy/HEAD/tpf3mp-native.json",
                file(vec![thief]),
            ),
            (
                "https://raw.githubusercontent.com/b/trees/HEAD/tpf3mp-native.json",
                file(vec![broken]),
            ),
            (
                "https://raw.githubusercontent.com/c/signals/HEAD/tpf3mp-native.json",
                file(vec![own_page]),
            ),
        ]
        .into();
        let built = build(
            "https://github.com/a/bigmap\nhttps://github.com/z/copy\n\
             https://github.com/b/trees\nhttps://github.com/c/signals\nhttps://github.com/d/gone\n",
            7,
            |url, _| files.get(url).cloned().ok_or_else(|| "404".to_owned()),
        );
        let ids: Vec<&str> = built.index.packages.iter().map(|p| p.id.as_str()).collect();
        assert_eq!(ids, ["bigmap", "signals"]);
        assert_eq!(built.index.serial, 7);
        assert_eq!(
            built.index.packages[0].homepage.as_deref(),
            Some("https://github.com/a/bigmap"),
            "the repository is the mod's page"
        );
        assert_eq!(
            built.index.packages[1].homepage.as_deref(),
            Some("https://example.com/signals")
        );
        assert_eq!(built.problems.len(), 3, "{:?}", built.problems);
        assert!(built.problems[0].contains("is https://github.com/a/bigmap's"));
        // What is served reads back as an index whose packages run here.
        let served = Index::parse(&built.json).unwrap();
        assert_eq!(served, built.index);
        assert!(served.packages[0].runs_on(BUILD));
    }

    #[test]
    fn a_mod_names_its_authors_and_preview_or_its_repository_does() {
        let mut named = package("bigmap", "0.3.0", &[]);
        named.authors = vec!["Ann".into(), "Bob".into()];
        named.preview = Some("images/preview.png".into());
        let mut plain = package("trees", "1.0.0", &[]);
        plain.preview = Some("https://example.com/trees.jpg".into());
        let mut escaping = package("signals", "1.0.0", &[]);
        escaping.preview = Some("../secret.png".into());
        let files: BTreeMap<&str, Vec<u8>> = [
            (
                "https://raw.githubusercontent.com/a/bigmap/HEAD/tpf3mp-native.json",
                file(vec![named]),
            ),
            (
                "https://raw.githubusercontent.com/b/trees/HEAD/tpf3mp-native.json",
                file(vec![plain]),
            ),
            (
                "https://raw.githubusercontent.com/c/signals/HEAD/tpf3mp-native.json",
                file(vec![escaping]),
            ),
        ]
        .into();
        let built = build(
            "https://github.com/a/bigmap\nhttps://github.com/b/trees\nhttps://github.com/c/signals\n",
            1,
            |url, _| files.get(url).cloned().ok_or_else(|| "404".to_owned()),
        );
        let [bigmap, trees] = &built.index.packages[..] else {
            panic!("{:?}", built.problems);
        };
        assert_eq!(bigmap.authors, ["Ann", "Bob"]);
        assert_eq!(
            bigmap.preview.as_deref(),
            Some("https://raw.githubusercontent.com/a/bigmap/HEAD/images/preview.png"),
            "a path is read from the mod's repository"
        );
        assert_eq!(trees.authors, ["b"], "the repository's owner by default");
        assert_eq!(
            trees.preview.as_deref(),
            Some("https://example.com/trees.jpg")
        );
        assert!(built.problems[0].contains("neither an https address nor a plain path"));
        assert_eq!(
            built.sources,
            BTreeMap::from([
                (
                    "bigmap".to_owned(),
                    "https://github.com/a/bigmap".to_owned()
                ),
                ("trees".to_owned(), "https://github.com/b/trees".to_owned()),
            ])
        );
    }

    #[test]
    fn github_says_how_often_a_mod_was_downloaded_and_when_it_changed() {
        let sources: BTreeMap<String, String> = [
            ("bigmap", "https://github.com/a/bigmap"),
            ("bigmap_extra", "https://github.com/a/bigmap"),
            ("trees", "https://github.com/b/trees"),
            ("gone", "https://github.com/c/gone"),
            ("elsewhere", "https://gitlab.com/d/elsewhere"),
        ]
        .into_iter()
        .map(|(id, repository)| (id.to_owned(), repository.to_owned()))
        .collect();
        let answers: BTreeMap<&str, &[u8]> = [
            (
                "https://api.github.com/repos/a/bigmap/releases?per_page=100",
                br#"[
                  {"published_at": "2026-09-30T10:00:00Z", "assets": [{"download_count": 1200}, {"download_count": 34}]},
                  {"published_at": "2026-08-01T10:00:00Z", "assets": [{"download_count": 66}]}
                ]"#
                .as_slice(),
            ),
            (
                "https://api.github.com/repos/b/trees/releases?per_page=100",
                b"[]".as_slice(),
            ),
            (
                "https://api.github.com/repos/b/trees",
                br#"{"pushed_at": "2026-10-02T08:00:00Z", "stargazers_count": 5}"#.as_slice(),
            ),
        ]
        .into();
        let asked = std::cell::RefCell::new(Vec::new());
        let (listings, problems) = listings(&sources, |url| {
            asked.borrow_mut().push(url.to_owned());
            answers
                .get(url)
                .map(|bytes| bytes.to_vec())
                .ok_or_else(|| "404".to_owned())
        });
        let bigmap = Listing {
            downloads: Some(1300),
            updated: Some("2026-09-30".into()),
        };
        assert_eq!(listings["bigmap"], bigmap);
        assert_eq!(listings["bigmap_extra"], bigmap, "one repository, one ask");
        assert_eq!(
            listings["trees"],
            Listing {
                downloads: None,
                updated: Some("2026-10-02".into()),
            },
            "no releases: the last push"
        );
        assert!(!listings.contains_key("gone") && !listings.contains_key("elsewhere"));
        assert_eq!(problems.len(), 1, "{problems:?}");
        assert_eq!(
            asked
                .borrow()
                .iter()
                .filter(|url| url.contains("a/bigmap"))
                .count(),
            1
        );

        // They reach the served index, and read back.
        let mut built = Built {
            index: Index {
                format: FORMAT,
                serial: 1,
                packages: Vec::new(),
                listings: Default::default(),
            },
            json: Vec::new(),
            problems: Vec::new(),
            sources,
        };
        built.add_listings(listings);
        assert_eq!(
            Index::parse(&built.json).unwrap().listings["bigmap"],
            bigmap
        );
    }

    #[test]
    fn open_pull_requests_propose_the_mods_they_add() {
        assert_eq!(
            pulls_url("https://github.com/tpf3mp/registry.git").unwrap(),
            "https://api.github.com/repos/tpf3mp/registry/pulls?state=open&per_page=50"
        );
        assert_eq!(pulls_url("https://gitlab.com/a/b"), None);
        let pulls = parse_pulls(
            br#"[
              {"number": 4, "title": "Add Trees", "html_url": "https://github.com/r/reg/pull/4",
               "head": {"sha": "abc123", "repo": {"full_name": "someone/reg"}}},
              {"number": 3, "title": "Gone fork", "html_url": "https://github.com/r/reg/pull/3",
               "head": {"sha": "def456", "repo": null}}
            ]"#,
        )
        .unwrap();
        assert_eq!(
            pulls.len(),
            1,
            "a pull request whose fork is gone is skipped"
        );
        assert_eq!(
            pulls[0].list_url,
            "https://raw.githubusercontent.com/someone/reg/abc123/mods.txt"
        );

        let accepted = Index {
            format: FORMAT,
            serial: 1,
            packages: vec![package("bigmap", "0.3.0", &[])],
            listings: Default::default(),
        };
        let list =
            "https://github.com/a/bigmap\nhttps://github.com/b/trees\nhttps://github.com/c/again\n";
        let files: BTreeMap<&str, Vec<u8>> = [
            (
                "https://raw.githubusercontent.com/someone/reg/abc123/mods.txt",
                list.as_bytes().to_vec(),
            ),
            (
                "https://raw.githubusercontent.com/b/trees/HEAD/tpf3mp-native.json",
                file(vec![package("trees", "1.0.0", &[])]),
            ),
            (
                "https://raw.githubusercontent.com/c/again/HEAD/tpf3mp-native.json",
                file(vec![package("bigmap", "5.0.0", &[])]),
            ),
        ]
        .into();
        let (proposals, json, problems) = propose(
            "https://github.com/a/bigmap\n",
            &accepted,
            &pulls,
            2,
            |url, _| files.get(url).cloned().ok_or_else(|| "404".to_owned()),
        );
        let ids: Vec<&str> = proposals.packages.iter().map(|p| p.id.as_str()).collect();
        assert_eq!(ids, ["trees"], "an accepted id is never proposed again");
        assert_eq!(proposals.pulls["trees"].number, 4);
        assert_eq!(problems.len(), 1);
        assert_eq!(Proposals::parse(&json).unwrap(), proposals);
    }

    #[test]
    fn serials_only_grow() {
        assert_eq!(next_serial(5, 1_000), 1_000);
        assert_eq!(next_serial(1_000, 1_000), 1_001);
        assert_eq!(next_serial(u64::MAX, 0), u64::MAX);
    }
}
