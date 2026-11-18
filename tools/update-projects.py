#!/usr/bin/env python3
"""Install a locally built proto-gen into adjacent projects' offline caches."""
import os
from pathlib import Path
import platform
import re
import shutil
import subprocess
import sys
import tempfile


def update(root, tag):
    if not re.fullmatch(r'v[0-9]+\.[0-9]+\.[0-9]+(?:[.-][0-9A-Za-z.-]+)?', tag):
        raise ValueError(f'无效的版本标签：{tag}')
    systems = {'Darwin': 'macos', 'Linux': 'linux'}
    architectures = {'arm64': 'aarch64', 'aarch64': 'aarch64', 'x86_64': 'x86_64', 'amd64': 'x86_64'}
    host = f'{systems[platform.system()]}-{architectures[platform.machine().lower()]}'
    binary = root / 'target/release/proto-gen'
    expected = f'proto-gen {tag[1:]}'
    if subprocess.check_output([str(binary), '--version'], text=True).strip() != expected:
        raise ValueError(f'本地程序版本与 {tag} 不一致')
    projects = []
    for name in ('node-core', 'aigc-php-core', 'aigc-php-core-co', 'go-core', 'rust-core'):
        project = root.parent / name
        pin = project / 'proto/proto-gen.version'
        if pin.is_file():
            projects.append((project, pin))
        else:
            print(f'跳过 {name}：未找到 {pin}', flush=True)
    if not projects:
        return

    # Reuse the host toolchain already installed by prepare.sh, including for
    # projects whose cache has not been populated yet. Never download here.
    companions = {}
    for name, local in (('protoc', root / 'toolchain/bin/protoc'),
                        ('protoc-gen-go', root / 'dist-tools/protoc-gen-go')):
        candidates = [local]
        for project, pin in projects:
            cache = project / '.tools/proto-gen'
            candidates.append(cache / pin.read_text().strip() / host / name)
            candidates.extend(sorted(cache.glob(f'*/{host}/{name}'), reverse=True))
        companions[name] = next((p for p in candidates if p.is_file() and os.access(p, os.X_OK)), None)
        if companions[name] is None:
            raise ValueError(f'缺少本机 {name}；请先准备本地工具链或至少一个业务项目的缓存')

    for project, pin in projects:
        cache = project / '.tools/proto-gen' / tag
        cache.mkdir(parents=True, exist_ok=True)
        destination = cache / host
        with tempfile.TemporaryDirectory(prefix='.local-update-', dir=cache) as temporary:
            stage = Path(temporary) / host
            stage.mkdir()
            for name, source in {'proto-gen': binary, **companions}.items():
                shutil.copy2(source, stage / name)
                (stage / name).chmod(0o755)
            shutil.copytree(root / 'schema', stage / 'schema')
            shutil.copytree(root / 'docs', stage / 'docs')
            shutil.copy2(root / 'README.md', stage / 'README.md')
            shutil.copy2(root / 'include/LICENSE.protobuf', stage / 'LICENSE.protobuf')
            (stage / 'version.txt').write_text(tag + '\n')
            subprocess.run([str(stage / 'proto-gen'), '--version'], check=True)
            backup = Path(temporary) / 'previous'
            if destination.exists():
                destination.rename(backup)
            try:
                stage.rename(destination)
            except OSError:
                if backup.exists():
                    backup.rename(destination)
                raise
        if pin.read_text() != tag + '\n':
            pin.write_text(tag + '\n')
        print(f'已更新 {project.name}：{destination / "proto-gen"}（{tag}）', flush=True)


if __name__ == '__main__':
    if len(sys.argv) != 3:
        sys.exit('用法：update-projects.py <proto-gen目录> <vX.Y.Z>')
    update(Path(sys.argv[1]).resolve(), sys.argv[2])
