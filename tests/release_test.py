#!/usr/bin/env python3
"""Exercise release version propagation offline, without real commits or pushes."""
import json
import os
from pathlib import Path
import platform
import shutil
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parents[1]
PROJECTS = ('node-core', 'aigc-php-core', 'aigc-php-core-co', 'go-core', 'rust-core')

with tempfile.TemporaryDirectory(prefix='proto release ') as directory:
    workspace = Path(directory)
    repo = workspace / 'proto-gen'
    (repo / 'tests').mkdir(parents=True)
    shutil.copy2(ROOT / 'release.sh', repo / 'release.sh')
    (repo / 'tests/install_test.py').write_text('')
    (repo / 'tests/release_test.py').write_text('')
    (repo / 'tools').mkdir()
    shutil.copy2(ROOT / 'tools/update-projects.py', repo / 'tools/update-projects.py')
    for directory in ('schema', 'docs', 'include', 'target/release'):
        (repo / directory).mkdir(parents=True)
    (repo / 'README.md').write_text('local build\n')
    (repo / 'include/LICENSE.protobuf').write_text('license\n')
    (repo / 'schema/field.proto').write_text('current schema\n')
    binary = repo / 'target/release/proto-gen'
    binary.write_text('#!/bin/sh\nprintf "proto-gen 1.2.3\\n"\n')
    binary.chmod(0o755)
    fake_bin = workspace / 'bin'
    fake_bin.mkdir()
    git = fake_bin / 'git'
    git.write_text('''#!/bin/sh
printf '%s\\n' "$1" >> "$RELEASE_TEST_GIT_LOG"
case "$1" in
 symbolic-ref) printf 'main\\n' ;;
 remote) printf 'https://example.invalid/proto-gen.git\\n' ;;
 ls-remote) if [ "$RELEASE_TEST_REMOTE_TAG" = 1 ]; then printf 'abc refs/tags/v1.2.3\\n'; fi ;;
 status) if [ "$RELEASE_TEST_DIRTY" = 1 ]; then printf ' M src/main.rs\\n'; fi ;;
 show-ref) exit 1 ;;
 push) exit "${RELEASE_TEST_PUSH_EXIT:-0}" ;;
esac
''')
    cargo = fake_bin / 'cargo'
    metadata = json.dumps({'packages': [{'manifest_path': str(repo / 'Cargo.toml'), 'version': '1.2.3'}]})
    cargo.write_text('#!/bin/sh\nprintf \'%s\\n\' "$*" >> "$RELEASE_TEST_CARGO_LOG"\nif [ "$1" = metadata ]; then\ncat <<\'JSON\'\n'
                     + metadata + '\nJSON\nfi\n')
    for executable in (git, cargo):
        executable.chmod(0o755)
    versions = []
    system = {'Darwin': 'macos', 'Linux': 'linux'}[platform.system()]
    arch = {'arm64': 'aarch64', 'aarch64': 'aarch64', 'x86_64': 'x86_64', 'amd64': 'x86_64'}[platform.machine().lower()]
    host = f'{system}-{arch}'
    for project in PROJECTS:
        version = workspace / project / 'proto/proto-gen.version'
        version.parent.mkdir(parents=True)
        version.write_text('v0.1.0\n')
        versions.append(version)
    # One existing cache supplies the toolchain for every project, including
    # projects without any cache. These tools must survive the local update.
    old_cache = workspace / PROJECTS[0] / '.tools/proto-gen/v0.1.0' / host
    old_cache.mkdir(parents=True)
    for name in ('protoc', 'protoc-gen-go'):
        tool = old_cache / name
        tool.write_text('#!/bin/sh\nexit 0\n')
        tool.chmod(0o755)
    unrelated = workspace / 'node-core/notes.txt'
    unrelated.write_text('keep local changes\n')

    git_log = workspace / 'git.log'
    cargo_log = workspace / 'cargo.log'

    def run(push_exit=0, remote_tag=False, dirty=False):
        git_log.write_text('')
        cargo_log.write_text('')
        env = dict(os.environ, PATH=str(fake_bin) + os.pathsep + os.environ['PATH'],
                   RELEASE_TEST_PUSH_EXIT=str(push_exit),
                   RELEASE_TEST_REMOTE_TAG=str(int(remote_tag)),
                   RELEASE_TEST_DIRTY=str(int(dirty)), RELEASE_TEST_GIT_LOG=str(git_log),
                   RELEASE_TEST_CARGO_LOG=str(cargo_log))
        return subprocess.run(['sh', str(repo / 'release.sh')], env=env, cwd=repo,
                              text=True, capture_output=True)

    failed = run(push_exit=1)
    assert failed.returncode != 0, failed.stdout
    assert all(p.read_text() == 'v0.1.0\n' for p in versions)
    assert not any((workspace / p / '.tools/proto-gen/v1.2.3').exists() for p in PROJECTS)

    binary.write_text('#!/bin/sh\nprintf "proto-gen 0.0.0\\n"\n')
    assert run().returncode != 0
    assert all(p.read_text() == 'v0.1.0\n' for p in versions)
    binary.write_text('#!/bin/sh\nprintf "proto-gen 1.2.3\\n"\n')
    plugin = old_cache / 'protoc-gen-go'
    plugin.rename(old_cache / 'plugin-backup')
    assert run().returncode != 0
    assert all(p.read_text() == 'v0.1.0\n' for p in versions)
    (old_cache / 'plugin-backup').rename(plugin)

    released = run()
    assert released.returncode == 0, released.stderr
    assert all(p.read_text() == 'v1.2.3\n' for p in versions)
    assert unrelated.read_text() == 'keep local changes\n'
    for project in PROJECTS:
        installed = workspace / project / '.tools/proto-gen/v1.2.3' / host
        assert (installed / 'proto-gen').read_bytes() == binary.read_bytes()
        assert (installed / 'version.txt').read_text() == 'v1.2.3\n'
        assert (installed / 'schema/field.proto').read_text() == 'current schema\n'
        for name in ('proto-gen', 'protoc', 'protoc-gen-go'):
            assert os.access(installed / name, os.X_OK)
    mtimes = [p.stat().st_mtime_ns for p in versions]
    assert run().returncode == 0
    assert [p.stat().st_mtime_ns for p in versions] == mtimes

    # An existing remote tag must refresh the actual binary and pins even when
    # current source is dirty, without committing, retagging or pushing.
    binary.write_text('#!/bin/sh\n# rebuilt current source\nprintf "proto-gen 1.2.3\\n"\n')
    for pin in versions:
        pin.write_text('v0.1.0\n')
    refreshed = run(remote_tag=True, dirty=True)
    assert refreshed.returncode == 0, refreshed.stderr
    assert '远端标签 v1.2.3 已存在' in refreshed.stdout
    assert 'build --release --bin proto-gen' in cargo_log.read_text().splitlines()
    assert all(p.read_text() == 'v1.2.3\n' for p in versions)
    for project in PROJECTS:
        installed = workspace / project / '.tools/proto-gen/v1.2.3' / host / 'proto-gen'
        assert installed.read_bytes() == binary.read_bytes()
    assert not {'add', 'commit', 'tag', 'push'}.intersection(git_log.read_text().splitlines())

    rejected = run(dirty=True)
    assert rejected.returncode != 0
    assert '请先提交' in rejected.stderr

    shutil.rmtree(workspace / PROJECTS[-1])
    missing = run()
    assert missing.returncode == 0, missing.stderr
    assert '跳过 rust-core' in missing.stdout
    assert not versions[-1].exists()

print('PASS: five local binaries and pins, cached toolchains, failed push, repeat update, missing project and unrelated edits')
