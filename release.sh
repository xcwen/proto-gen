#!/bin/sh
# Change only [package].version in Cargo.toml, then run ./release.sh.
set -eu

die() {
    printf '发布失败：%s\n' "$*" >&2
    exit 1
}

case "${1:-}" in
    -h|--help)
        printf '%s\n' '用法：./release.sh' \
            '读取 Cargo.toml 版本，同步 Cargo.lock，运行检查，提交并推送 main 和版本标签。' \
            'GitHub Actions 随后构建四个平台并发布 Release。其他代码改动请先提交。'
        exit 0
        ;;
    '') ;;
    *) die '不接受版本参数，请修改 Cargo.toml 中的 [package].version。' ;;
esac
[ "$#" -eq 0 ] || die '不接受额外参数。'

RELEASE_ROOT=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
cd "$RELEASE_ROOT"
for RELEASE_COMMAND in git cargo python3; do
    command -v "$RELEASE_COMMAND" >/dev/null 2>&1 || die "缺少命令：$RELEASE_COMMAND"
done

RELEASE_BRANCH=$(git symbolic-ref --quiet --short HEAD) || die '请切换到 main 分支。'
[ "$RELEASE_BRANCH" = main ] || die '请在 main 分支发布。'
git remote get-url origin >/dev/null || die '未配置 origin 远端。'

# Keep unrelated work out of the release commit, including already staged files.
RELEASE_OTHER_CHANGES=$(git status --porcelain=v1 --untracked-files=all -- . \
    ':(top,exclude)Cargo.toml' ':(top,exclude)Cargo.lock' ':(top,exclude)release.sh')
if [ -n "$RELEASE_OTHER_CHANGES" ]; then
    printf '%s\n' "$RELEASE_OTHER_CHANGES" >&2
    die '请先提交以上代码改动，再运行发布；脚本只自动提交 Cargo.toml、Cargo.lock 和 release.sh。'
fi

RELEASE_METADATA=$(cargo metadata --no-deps --format-version 1)
RELEASE_VERSION=$(printf '%s\n' "$RELEASE_METADATA" | python3 -c '
import json
from pathlib import Path
import re
import sys

metadata = json.load(sys.stdin)
manifest = Path("Cargo.toml").resolve()
package = next(p for p in metadata["packages"] if Path(p["manifest_path"]).resolve() == manifest)
version = package["version"]
if not re.fullmatch(r"[0-9]+\.[0-9]+\.[0-9]+([.-][0-9A-Za-z.-]+)?", version):
    sys.exit("版本不符合发布标签格式，请使用 x.y.z 或 x.y.z-预发布后缀。")
print(version)
')
RELEASE_TAG="v$RELEASE_VERSION"
printf '准备发布 proto-gen %s\n' "$RELEASE_TAG"

RELEASE_REMOTE_TAG=$(git ls-remote --tags origin "refs/tags/$RELEASE_TAG")
[ -z "$RELEASE_REMOTE_TAG" ] || die "远端标签 $RELEASE_TAG 已存在，请升级 Cargo.toml 版本。"

RELEASE_REUSE_TAG=0
if git show-ref --verify --quiet "refs/tags/$RELEASE_TAG"; then
    RELEASE_TAG_COMMIT=$(git rev-parse "$RELEASE_TAG^{commit}")
    [ "$RELEASE_TAG_COMMIT" = "$(git rev-parse HEAD)" ] || die "本地标签 $RELEASE_TAG 未指向当前提交。"
    [ -z "$(git status --porcelain=v1 --untracked-files=all)" ] || die "本地标签 $RELEASE_TAG 已存在，但工作区仍有改动。"
    RELEASE_REUSE_TAG=1
    printf '复用上次未推送的本地标签 %s\n' "$RELEASE_TAG"
fi

printf '%s\n' '检查格式并同步 Cargo.lock...'
cargo fmt --all --check
# The first check deliberately omits --locked so a package version bump updates the lockfile.
cargo check
printf '%s\n' '运行测试、Clippy 和安装器检查...'
cargo test --locked
cargo clippy --all-targets --locked -- -D warnings
python3 tests/install_test.py

if [ "$RELEASE_REUSE_TAG" -eq 0 ]; then
    git add -- Cargo.toml Cargo.lock release.sh
    if ! git diff --cached --quiet; then
        git commit -m "Release $RELEASE_TAG" --only -- Cargo.toml Cargo.lock release.sh
    fi
    git tag -a "$RELEASE_TAG" -m "Release $RELEASE_TAG"
else
    [ -z "$(git status --porcelain=v1 --untracked-files=all)" ] || die '检查后工作区有改动，无法复用现有标签。'
fi

printf '推送 main 和 %s，触发 GitHub Actions 发布...\n' "$RELEASE_TAG"
# Publish the branch and tag together; a failed push can be retried with the same script.
git push --atomic origin 'HEAD:refs/heads/main' "refs/tags/$RELEASE_TAG"
printf '已推送 %s；GitHub Actions 正在检查和构建，完成后生成 Release。\n' "$RELEASE_TAG"
printf '%s\n' '查看进度：https://github.com/xcwen/proto-gen/actions/workflows/release.yml'
