use super::*;
use serde_json::json;

pub(super) async fn prepare(rpc: &mut rpc::Rpc, spec: &JobSpec) -> Result<(), ProviderError> {
    if spec.kind == JobKind::JudgeWork {
        return Ok(());
    }
    let repo = spec.target.checkout().ok_or_else(|| {
        fault(
            JobFailureKind::Configuration,
            "workspace/prepare",
            "missing checkout target",
        )
    })?;
    for sha in [&repo.head_sha.0, &repo.base_sha.0] {
        if !matches!(sha.len(), 40 | 64) || !sha.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(fault(
                JobFailureKind::CheckoutFailed,
                "workspace/prepare",
                "invalid frozen SHA",
            ));
        }
    }
    let mirror = crate::git::mirror_path(std::path::Path::new("/mirror"), &repo.clone_url);
    // Retain only objects; rebuild the index to discard agent-writable entry flags and stat data.
    rpc.command(json!({"command":["bash","-c", r#"
set -eu
export GIT_CONFIG_GLOBAL=/dev/null GIT_CONFIG_NOSYSTEM=1 GIT_ATTR_NOSYSTEM=1
git_safe() { git -c safe.directory='*' -c core.hooksPath=/dev/null -c submodule.recurse=false -c core.sparseCheckout=false "$@"; }
if [ -d /work/repo/.git/objects ] && [ ! -L /work/repo ] && [ ! -L /work/repo/.git ] && [ ! -L /work/repo/.git/objects ]; then
    saved=$(mktemp -d /work/git-reset.XXXXXX)
    trap 'rm -rf -- "$saved"' EXIT
    mv /work/repo/.git/objects "$saved/objects"
    rm -rf /work/repo/.git
    object_format=$(git_safe -C "$1" rev-parse --show-object-format)
    git_safe init --quiet --template= --object-format="$object_format" /work/repo
    rm -rf /work/repo/.git/objects
    mv "$saved/objects" /work/repo/.git/objects
    rm -rf /work/repo/.git/objects/info
    rm -f /work/repo/.git/objects/pack/multi-pack-index
    mkdir /work/repo/.git/objects/info
else
    rm -rf /work/repo
    git_safe clone --quiet --no-checkout --no-hardlinks --template= -- "$1" /work/repo
fi
git_safe -C /work/repo fetch --quiet --no-tags --no-recurse-submodules -- "$1" "$2" "$3"
git_safe -C /work/repo checkout --quiet --force --detach "$2"
git_safe -C /work/repo clean --quiet -ffdx
git_safe -C /work/repo cat-file -e "$3^{commit}"
"#,
        "diffpilot-checkout",mirror,repo.head_sha.0,repo.base_sha.0],"cwd":"/work","timeoutMs":60_000,"outputBytesCap":4096})).await?;
    for path in ["/work/out", "/work/pr-data", "/work/flow-context.json"] {
        rpc.remove(path).await?;
    }
    rpc.mkdir("/work/out").await?;
    if let Some(flow) = &spec.flow_file {
        rpc.write(
            "/work/flow-context.json",
            flow.context.to_string().as_bytes(),
        )
        .await?;
        rpc.command(json!({"command":["chmod","0444","/work/flow-context.json"],"cwd":"/work"}))
            .await?;
        if let Some(previous) = &flow.previous {
            rpc.write("/work/out/flow.md", previous.as_bytes()).await?;
        }
    }
    if let Some(data) = &spec.pr_data {
        rpc.mkdir("/work/pr-data").await?;
        for (name, value) in [
            ("manifest", serde_json::to_value(&data.manifest)),
            ("threads", Ok(data.threads.clone())),
            (
                "review-comments",
                serde_json::to_value(&data.review_comments),
            ),
            ("reviews", serde_json::to_value(&data.reviews)),
            ("comments", serde_json::to_value(&data.comments)),
            ("ids", serde_json::to_value(&data.ids)),
            ("scope", serde_json::to_value(&data.scope)),
        ] {
            let value = value.map_err(|_| {
                fault(
                    JobFailureKind::Configuration,
                    "workspace/prepare",
                    "PR data serialization",
                )
            })?;
            rpc.write(
                &format!("/work/pr-data/{name}.json"),
                value.to_string().as_bytes(),
            )
            .await?;
        }
        for (path, text) in &data.conventions {
            let relative = std::path::Path::new(path);
            if path.is_empty()
                || !relative
                    .components()
                    .all(|c| matches!(c, std::path::Component::Normal(_)))
            {
                return Err(fault(
                    JobFailureKind::Configuration,
                    "workspace/prepare",
                    "invalid convention path",
                ));
            }
            let path = format!("/work/pr-data/conventions/{path}");
            if let Some((parent, _)) = path.rsplit_once('/') {
                rpc.mkdir(parent).await?;
            }
            rpc.write(&path, text.as_bytes()).await?;
        }
    }
    Ok(())
}

pub(super) async fn hygiene(rpc: &mut rpc::Rpc, spec: &JobSpec) -> Result<(), ProviderError> {
    if spec.kind == JobKind::JudgeWork {
        let preset = include_str!("../../../../../deploy/worker/config.toml")
            .replace(
                "sandbox_mode = \"danger-full-access\"",
                "sandbox_mode = \"read-only\"",
            )
            .replace(
                "features = {",
                "features = { shell_tool = false, unified_exec = false, view_image = false,",
            );
        // W3 Ensure creates /codex/config.toml -> /run/dp/config.toml on tmpfs.
        rpc.write("/run/dp/config.toml", preset.as_bytes()).await?;
        return Ok(());
    }
    let env = match &spec.access.memcan {
        Some(endpoint) => {
            json!({"DP_MEMCAN_URL":endpoint.url,"DP_MEMCAN_TOKEN":endpoint.token.expose()})
        }
        None => json!({}),
    };
    rpc.command(json!({"command":["python3","/usr/local/bin/worker-config.py","cs1"],"cwd":"/work","env":env,"timeoutMs":10_000,"outputBytesCap":0})).await?;
    let skills = rpc
        .request("skills/list", json!({"cwds":["/work"],"forceReload":true}))
        .await?;
    for entry in skills["data"].as_array().into_iter().flatten() {
        for skill in entry["skills"].as_array().into_iter().flatten() {
            if skill["scope"] == "repo" {
                let path = skill["path"].as_str().ok_or_else(|| {
                    fault(
                        JobFailureKind::Configuration,
                        "skills/list",
                        "repository skill has no path",
                    )
                })?;
                rpc.request("skills/config/write", json!({"path":path,"enabled":false}))
                    .await?;
            }
        }
    }
    Ok(())
}
