#!/usr/bin/env python3
"""List, verify and project previously exposed public repository development questions."""
import argparse
import json
import os
from pathlib import Path
import re
import sys

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parent))
from public_reference import inspect_git_tree, inspect_publication  # noqa: E402

MANIFEST = HERE / 'development-cases.json'
KINDS = {'positive', 'absent_path', 'absent_in_file', 'absent_namespace',
         'unavailable_runtime', 'unavailable_future'}


def verify_span(source, witness):
    start, end = witness['startLine'], witness['endLine']
    lines = source.splitlines(keepends=True)
    if not isinstance(start, int) or not isinstance(end, int) or not 1 <= start <= end <= len(lines):
        raise ValueError('Invalid source line range')
    expected = witness['text'].encode('utf-8')
    if 'startByte' in witness or 'endByte' in witness:
        lo, hi = witness['startByte'], witness['endByte']
        if not 0 <= lo < hi <= len(source) or source[lo:hi] != expected:
            raise ValueError('Source byte range differs from frozen witness')
        if source[:lo].count(b'\n') + 1 != start or source[:hi].count(b'\n') + (not expected.endswith(b'\n')) != end:
            raise ValueError('Source line coordinates differ from frozen witness')
    elif b''.join(lines[start - 1:end]) != expected:
        raise ValueError('Source line text differs from frozen witness')


def witnesses(case):
    """Yield every supplied alternative and all of its conjunctive anchors."""
    if 'witness' in case:
        yield case['witness']
    yield from case.get('witnesses', [])
    for fact in case.get('facts', []):
        for alternative in fact['sufficientAlternatives']:
            yield from alternative


def verify_absence(source, absence):
    if re.search(absence['pattern'], source.decode('utf-8')):
        raise ValueError('Absence condition does not hold in complete file')


def read_manifest(path):
    value = json.loads(path.read_text(encoding='utf-8'))
    if value.get('version') != 1 or not isinstance(value.get('cases'), list):
        raise ValueError('Invalid development manifest')
    ids = set()
    for case in value['cases']:
        if case['id'] in ids or case['kind'] not in KINDS or case['repository'] not in value['references']:
            raise ValueError('Duplicate case ID or invalid case kind/repository')
        if not case.get('query') or not case.get('expectation') or case.get('language') not in ('en', 'fr'):
            raise ValueError('Incomplete case')
        if case['kind'] in ('positive', 'unavailable_runtime', 'unavailable_future', 'absent_namespace') and not list(witnesses(case)):
            raise ValueError('Case has no source witness')
        if case['kind'] == 'positive' and case.get('facts') is not None:
            if not case['facts'] or any(not f.get('sufficientAlternatives') or any(not a for a in f['sufficientAlternatives']) for f in case['facts']):
                raise ValueError('Incomplete fact alternatives')
        if case['kind'] == 'absent_namespace' and not case.get('absentNamespace'):
            raise ValueError('Missing absent namespace')
        if case['kind'] == 'absent_in_file' and not {'witness', 'absence'} <= case.keys():
            raise ValueError('Missing file absence evidence')
        if case['kind'] == 'absent_path' and not case.get('absentPath'):
            raise ValueError('Missing absent path')
        ids.add(case['id'])
    if not ids:
        raise ValueError('Empty manifest')
    return value


def reference_path(manifest, name, manifest_path=MANIFEST):
    try:
        relative = Path(manifest['references'][name])
    except KeyError as error:
        raise ValueError(f'Unknown repository: {name}') from error
    path = Path(manifest_path).resolve().parent / relative
    legacy = {'commons-current': 'commons-cli-current-reference.json',
              'jackson': 'jackson-databind-reference.json'}
    if not path.exists() and str(relative) == legacy.get(name):
        return HERE / relative
    return path


def git_stores(manifest, selected, direct, assignments, legacy):
    paths = {name: path for name, path in legacy.items() if path is not None}
    if direct is not None:
        if selected is None:
            raise ValueError('--git requires --repository')
        assignments = [*assignments, f'{selected}={direct}']
    for entry in assignments:
        name, mark, path = entry.partition('=')
        if not mark or not name or not path or name in paths:
            raise ValueError(f'Invalid or duplicate Git store assignment: {entry}')
        paths[name] = Path(path)
    known = manifest['references']
    if selected is not None and selected not in known:
        raise ValueError(f'Unknown repository: {selected}')
    unknown = paths.keys() - known.keys()
    if unknown:
        raise ValueError(f"Unknown repository: {', '.join(sorted(unknown))}")
    required = (selected,) if selected is not None else known
    missing = [name for name in required if name not in paths]
    if missing:
        raise ValueError(f"Missing local Git store: {', '.join(missing)}")
    return paths


def verify(value, repos, selected=None, *, manifest_path=MANIFEST):
    # No implicit network or object replacement even if Git configuration permits it.
    os.environ['GIT_NO_LAZY_FETCH'] = '1'
    os.environ['GIT_NO_REPLACE_OBJECTS'] = '1'
    if selected is not None and selected not in value['references']:
        raise ValueError(f'Unknown repository: {selected}')
    snapshots = {}
    for name in value['references']:
        if selected is not None and name != selected:
            continue
        ref = json.loads(reference_path(value, name, manifest_path).read_text(encoding='utf-8'))
        if repos.get(name) is None:
            raise ValueError(f'Missing local Git store: {name}')
        git = inspect_git_tree(repos[name], ref)
        inspect_publication(git, ref)
        snapshots[name] = (git, ref)
    checked_cases = 0
    for case in value['cases']:
        if selected is not None and case['repository'] != selected:
            continue
        checked_cases += 1
        git, ref = snapshots[case['repository']]
        files = git.files[ref['commit']]
        for evidence in witnesses(case):
            entry = files.get(evidence['path'])
            if entry is None or entry['sha'] != evidence['blob'] or evidence.get('revision', ref['commit']) != ref['commit']:
                raise ValueError(f"{case['id']}: source blob or revision differs")
            verify_span(git.blob(entry['sha']), evidence)
        if 'absence' in case:
            evidence = case['absence']
            entry = files.get(evidence['path'])
            if entry is None or entry['sha'] != evidence['blob']:
                raise ValueError(f"{case['id']}: absence blob differs")
            verify_absence(git.blob(entry['sha']), evidence)
        if case['kind'] == 'absent_namespace':
            # Search all tracked Java blobs, not only the expected directory.
            pattern = re.compile(r'\bpackage\s+' + re.escape(case['absentNamespace']) + r'(?:\s*;|\.)')
            for path, entry in files.items():
                if path.endswith('.java') and pattern.search(git.blob(entry['sha']).decode('utf-8')):
                    raise ValueError(f"{case['id']}: expected absent namespace exists")
        if case['kind'] == 'absent_path' and case['absentPath'] in files:
            raise ValueError(f"{case['id']}: expected absent path exists")
    if checked_cases == 0:
        raise ValueError('No cases for selected repository')
    return {'status': 'verified', 'cases': checked_cases,
            'repositories': {name: ref['commit'] for name, (_, ref) in snapshots.items()}}


def projection(manifest, repo, store, *, manifest_path=MANIFEST):
    """Return only the selected fixture identity and question inputs."""
    cases = [{**{key: case[key] for key in ('id', 'language', 'query')}, 'reference': repo}
             for case in manifest['cases'] if case['repository'] == repo]
    if not cases:
        raise ValueError('No cases for selected repository')
    return {'references': {repo: {'gitDirectory': str(store.resolve()),
            'reference': str(reference_path(manifest, repo, manifest_path).resolve()),
            'branch': manifest['fixtureBranches'][repo]}}, 'cases': cases}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('action', choices=('list', 'verify', 'project'))
    parser.add_argument('--manifest', type=Path, default=MANIFEST, help='Development manifest (defaults to the checked-in file).')
    parser.add_argument('--repository', help='Select a repository key defined by the manifest.')
    parser.add_argument('--git', type=Path, help='Local bare Git store for --repository.')
    parser.add_argument('--git-store', metavar='REPOSITORY=PATH', action='append', default=[],
                        help='Assign a local Git store to a manifest repository (repeatable).')
    parser.add_argument('--commons-git', type=Path, help='Local bare Git store for the pinned Commons CLI revision.')
    parser.add_argument('--jackson-git', type=Path, help='Local bare Git store for the pinned Jackson revision.')
    args = parser.parse_args()
    try:
        manifest = read_manifest(args.manifest)
        if args.repository is not None and args.repository not in manifest['references']:
            raise ValueError(f'Unknown repository: {args.repository}')
        if args.action == 'list':
            result = {'exposure': manifest['exposure'], 'cases': [
                {key: c[key] for key in ('id', 'language', 'query', 'repository', 'kind')}
                for c in manifest['cases'] if args.repository is None or c['repository'] == args.repository]}
        else:
            if args.action == 'project' and args.repository is None:
                parser.error('project requires --repository')
            paths = git_stores(manifest, args.repository, args.git, args.git_store,
                               {'commons-current': args.commons_git, 'jackson': args.jackson_git})
            checked = verify(manifest, paths, args.repository, manifest_path=args.manifest)
            if args.action == 'verify':
                result = checked
            else:
                result = projection(manifest, args.repository, paths[args.repository], manifest_path=args.manifest)
        print(json.dumps(result, ensure_ascii=False, indent=2))
    except (KeyError, OSError, ValueError, UnicodeError, RuntimeError) as error:
        parser.exit(1, f'verification failed: {error}\n')


if __name__ == '__main__':
    main()
