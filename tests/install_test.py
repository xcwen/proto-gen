#!/usr/bin/env python3
"""Offline release installer tests: platform mapping, checksums, cache and failed installs."""
import hashlib
import os
from pathlib import Path
import subprocess
import tarfile
import tempfile

ROOT = Path(__file__).resolve().parents[1]
with tempfile.TemporaryDirectory(prefix='proto-installer-') as directory:
    work = Path(directory)
    mirror = work / 'releases' / 'v0.1.0'
    mirror.mkdir(parents=True)
    fake_bin = work / 'bin'
    fake_bin.mkdir()
    uname = fake_bin / 'uname'
    uname.write_text('''#!/bin/sh
case "$1" in
 -s) printf '%s\\n' "$INSTALL_TEST_SYSTEM" ;;
 -m) printf '%s\\n' "$INSTALL_TEST_ARCH" ;;
 *) /usr/bin/uname "$@" ;;
esac
''')
    uname.chmod(0o755)
    cache = work / 'cache with spaces'
    sums = []
    platforms = [('Darwin', 'arm64', 'macos-aarch64'), ('Darwin', 'x86_64', 'macos-x86_64'),
                 ('Linux', 'aarch64', 'linux-aarch64'), ('Linux', 'x86_64', 'linux-x86_64')]
    for _, _, label in platforms:
        package = f'proto-gen-v0.1.0-{label}'
        source = work / package
        source.mkdir()
        for name in ['proto-gen', 'protoc', 'protoc-gen-go']:
            binary = source / name
            binary.write_text('#!/bin/sh\nprintf "proto-gen 0.1.0\\n"\n')
            binary.chmod(0o755)
        (source / 'version.txt').write_text('v0.1.0\n')
        archive = mirror / f'{package}.tar.gz'
        with tarfile.open(archive, 'w:gz') as stream:
            stream.add(source, arcname=package)
        sums.append(f'{hashlib.sha256(archive.read_bytes()).hexdigest()}  ./{archive.name}\n')
    (mirror / 'SHA256SUMS').write_text(''.join(sums))

    def run(system='Darwin', arch='arm64', success=True, base=None, version='v0.1.0'):
        env = dict(os.environ, PATH=str(fake_bin) + os.pathsep + os.environ['PATH'],
                   INSTALL_TEST_SYSTEM=system, INSTALL_TEST_ARCH=arch,
                   PROTO_GEN_RELEASE_BASE_URL=base or (work / 'releases').as_uri())
        result = subprocess.run(['sh', str(ROOT / 'install.sh'), version, str(cache)],
                                env=env, text=True, capture_output=True)
        assert (result.returncode == 0) == success, result.stderr
        return result

    for system, arch, label in platforms:
        result = run(system, arch)
        executable = Path(result.stdout.strip())
        assert executable == cache / 'v0.1.0' / label / 'proto-gen'
        assert os.access(executable, os.X_OK)
        assert subprocess.check_output([str(executable), '--version'], text=True).strip() == 'proto-gen 0.1.0'
        # A cached release needs no network, even if the configured mirror is unavailable.
        assert run(system, arch, base='file:///not-present').stdout == result.stdout

    run(system='Windows', success=False)
    run(arch='mips', success=False)
    run(version='v0.1.0/../../escape', success=False)
    # Corrupt checksum must leave the existing directory untouched.
    destination = cache / 'v0.1.0' / 'macos-aarch64'
    (destination / 'version.txt').write_text('incomplete')
    previous = (destination / 'proto-gen').read_bytes()
    (mirror / 'SHA256SUMS').write_text('0' * 64 + '  proto-gen-v0.1.0-macos-aarch64.tar.gz\n')
    result = run(success=False)
    assert 'checksum verification failed' in result.stderr
    assert (destination / 'proto-gen').read_bytes() == previous
    assert (destination / 'version.txt').read_text() == 'incomplete'
    assert not list((cache / 'v0.1.0').glob('.install-*'))
    print('PASS: four platforms, verified download, offline cache, invalid inputs and checksum failure')
