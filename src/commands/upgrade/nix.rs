//! Guidance for Nix installs. aix never modifies a Nix-managed binary: the
//! version is pinned by the user's flake or profile, so the upgrade has to go
//! through Nix.

use std::path::{Path, PathBuf};

/// Where a Nix-installed aix most likely came from.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum NixOrigin {
    /// Listed in `nix profile list` under this name.
    Profile { name: String },
    /// Anything else: home-manager module, overlay, NixOS/nix-darwin config,
    /// or `nix run`. They cannot be told apart from the store path alone.
    Declarative,
}

pub(crate) fn origin(store_path: &Path) -> NixOrigin {
    profile_manifests()
        .iter()
        .filter_map(|manifest| std::fs::read_to_string(manifest).ok())
        .find_map(|text| profile_element_name(&text, store_path))
        .map_or(NixOrigin::Declarative, |name| NixOrigin::Profile { name })
}

fn profile_manifests() -> Vec<PathBuf> {
    let Some(home) = directories::BaseDirs::new().map(|dirs| dirs.home_dir().to_path_buf()) else {
        return Vec::new();
    };
    let mut manifests = vec![home.join(".nix-profile").join("manifest.json")];
    if let Some(state) = std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .or_else(|| Some(home.join(".local").join("state")))
    {
        manifests.push(state.join("nix").join("profile").join("manifest.json"));
    }
    manifests
}

/// Find the `nix profile` element that installed `store_path`. Newer Nix keys
/// elements by name; older manifests are a list and are matched by index.
fn profile_element_name(manifest: &str, store_path: &Path) -> Option<String> {
    let manifest: serde_json::Value = serde_json::from_str(manifest).ok()?;
    let store_path = store_path.to_string_lossy();
    let owns = |element: &serde_json::Value| {
        element
            .get("storePaths")
            .and_then(serde_json::Value::as_array)
            .is_some_and(|paths| paths.iter().any(|path| path.as_str() == Some(&store_path)))
    };
    match manifest.get("elements")? {
        serde_json::Value::Object(elements) => elements
            .iter()
            .find(|(_, element)| owns(element))
            .map(|(name, _)| name.clone()),
        serde_json::Value::Array(elements) => elements
            .iter()
            .position(owns)
            .map(|index| index.to_string()),
        _ => None,
    }
}

pub(crate) fn guidance(store_path: &Path, origin: &NixOrigin) -> String {
    let mut text = format!(
        "\
aix is installed through Nix ({}).

Nix pins the exact version you get, so `aix upgrade` leaves this binary alone:
changing it behind Nix's back would be undone at the next rebuild anyway.
Upgrade it the Nix way instead.
",
        store_path.display()
    );

    match origin {
        NixOrigin::Profile { name } => text.push_str(&format!(
            "
You installed aix with `nix profile install`, so this is all you need:

    nix profile upgrade {name}
"
        )),
        NixOrigin::Declarative => text.push_str(
            "
If aix comes from a flake (home-manager module, overlay, or a NixOS /
nix-darwin configuration), bump the flake input and rebuild. In the directory
that holds your flake.nix:

    nix flake update aix        # use the input name you gave aix in flake.nix

then rebuild the way you normally do, for example:

    home-manager switch --flake .          # standalone home-manager
    sudo nixos-rebuild switch --flake .    # NixOS
    darwin-rebuild switch --flake .        # nix-darwin

On Nix older than 2.19, the first command is spelled
`nix flake lock --update-input aix`.

If you started it with `nix run`, add --refresh to fetch the newest version:

    nix run --refresh github:get-aix/aix -- --version

If you installed it with `nix profile install`, run `nix profile list` to find
its name, then `nix profile upgrade <name>`.
",
        ),
    }

    text.push_str(
        "
New to Nix? The flake.lock file next to flake.nix records the exact commit of
every input. `nix flake update` moves it to the newest commit, and the rebuild
installs what it points to. Nothing changes until you rebuild.
",
    );
    text.push_str(&format!(
        "More: {}/blob/main/docs/nix.md#upgrading\n",
        super::release::REPO_URL
    ));
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    const STORE: &str = "/nix/store/abc-aix-rs-1.0.0";

    #[test]
    fn finds_named_profile_elements() {
        let manifest = format!(
            r#"{{"version":3,"elements":{{"ripgrep":{{"storePaths":["/nix/store/x-rg"]}},"aix":{{"storePaths":["{STORE}"]}}}}}}"#
        );
        assert_eq!(
            profile_element_name(&manifest, Path::new(STORE)).as_deref(),
            Some("aix")
        );
    }

    #[test]
    fn falls_back_to_index_for_old_manifests() {
        let manifest = format!(
            r#"{{"version":2,"elements":[{{"storePaths":["/nix/store/x-rg"]}},{{"storePaths":["{STORE}"]}}]}}"#
        );
        assert_eq!(
            profile_element_name(&manifest, Path::new(STORE)).as_deref(),
            Some("1")
        );
    }

    #[test]
    fn home_manager_paths_are_not_profile_elements() {
        let manifest =
            r#"{"version":3,"elements":{"home-manager-path":{"storePaths":["/nix/store/hm"]}}}"#;
        assert_eq!(profile_element_name(manifest, Path::new(STORE)), None);
    }

    #[test]
    fn guidance_is_specific_for_profiles_and_general_otherwise() {
        let profile = guidance(Path::new(STORE), &NixOrigin::Profile { name: "aix".into() });
        assert!(profile.contains("nix profile upgrade aix"));
        assert!(!profile.contains("nixos-rebuild"));

        let declarative = guidance(Path::new(STORE), &NixOrigin::Declarative);
        assert!(declarative.contains("nix flake update aix"));
        assert!(declarative.contains("home-manager switch"));
        assert!(declarative.contains("nix run --refresh"));
    }
}
