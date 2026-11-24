#!/usr/bin/env python3
"""Conservative byte coverage of frozen source anchors by published verified quotes.

Coverage is not answer correctness. Uncovered anchors may still have an equivalent
proof and always require human review; this deliberately does not award such proof.
"""
import argparse
import json
from pathlib import Path
import re

from development_cases import read_manifest, verify, verify_span, reference_path
from public_reference import inspect_git_tree


def _quote_ranges(reopened, git, revision):
    result = {}
    for item in reopened:
        if item.get('verified') is not True:
            continue
        try:
            c = item['citation']
            evidence = c['evidence']
            loc = evidence['location']
            path = loc['path']
            entry = git.files[revision][path]
            if evidence['revision'] != revision or loc['blob'] != entry['sha']:
                continue
            source = git.blob(entry['sha'])
            lo = loc['startByte']
            hi = loc['endByte']
            if not 0 <= lo < hi <= len(source):
                continue
            excerpt = source[lo:hi]
            quote = c['quote'].encode('utf-8')
            if not quote or item.get('excerptUtf8', '').encode('utf-8') != excerpt:
                continue
            positions = [m.start() for m in re.finditer(b'(?=' + re.escape(quote) + b')', excerpt)]
            matches = []
            for pos in positions:
                start = lo + pos
                end = start + len(quote)
                first = source[:start].count(b'\n') + 1
                last = first + quote.removesuffix(b'\n').count(b'\n')
                fragment = f'L{first}' if first == last else f'L{first}-L{last}'
                if c.get('lineUrl') == loc['fileUrl'] + '#' + fragment:
                    matches.append((start, end))
            # A line link may still identify multiple copies on the same line.
            if len(matches) == 1:
                result.setdefault((path, entry['sha']), []).append(matches[0])
        except (KeyError, TypeError, ValueError, UnicodeError):
            continue
    return result


def _anchor_covered(anchor, ranges, git, revision):
    path = anchor['path']
    entry = git.files[revision].get(path)
    if not entry or anchor.get('revision', revision) != revision or anchor['blob'] != entry['sha']:
        raise ValueError('Frozen anchor identity differs from pinned Git')
    source = git.blob(entry['sha'])
    verify_span(source, anchor)
    lines = source.splitlines(keepends=True)
    lo = anchor.get('startByte', sum(map(len, lines[:anchor['startLine'] - 1])))
    hi = anchor.get('endByte', lo + len(anchor['text'].encode('utf-8')))
    spans = ranges.get((path, entry['sha']), [])
    # Only leading/trailing whitespace on each source line may be omitted.
    # Every byte inside the line, including string whitespace, must be quoted.
    cursor = lo
    for line in source[lo:hi].splitlines(keepends=True):
        content = line.strip(b' \t\r\n')
        if content:
            left = cursor + len(line) - len(line.lstrip(b' \t\r\n'))
            right = left + len(content)
            if not all(any(a <= i < b for a, b in spans) for i in range(left, right)):
                return False
        cursor += len(line)
    return True


def _fact_coverage(case, ranges, git, revision):
    facts = case.get('facts')
    if facts is None:
        anchors = ([case['witness']] if 'witness' in case else []) + case.get('witnesses', [])
        facts = [{'sufficientAlternatives': [anchors]}] if anchors else []
    results = []
    for fact in facts:
        alternatives = [[_anchor_covered(a, ranges, git, revision) for a in alt]
                        for alt in fact['sufficientAlternatives']]
        results.append({'covered': any(all(alt) for alt in alternatives),
                        'alternatives': alternatives})
    return results


def coverage(case, reopened, git, revision, answer):
    if case.get('kind') != 'positive':
        return {'status': 'not_applicable', 'meaning': 'Controls require human judgment.'}
    text = answer.get('text') if isinstance(answer, dict) else None
    published = []
    if (isinstance(text, str) and text.strip() and answer.get('outcome') == 'answered'
            and isinstance(answer.get('citations'), list)):
        published = answer['citations']
    attached = [item for item in reopened if item.get('citation') in published]
    ranges = _quote_ranges(attached, git, revision)
    results = _fact_coverage(case, ranges, git, revision)
    complete = bool(results) and all(x['covered'] for x in results)
    return {'status': 'anchors_covered' if complete else 'human_review', 'facts': results,
            'meaning': 'Anchor coverage only, not factual correctness; equivalent evidence requires human review.'}


def _context_ranges(sources, git, ref, branch):
    revision = ref['commit']
    ranges = {}
    slots = set()
    for source in sources:
        if not isinstance(source, dict):
            raise ValueError('Invalid context source')
        slot, kind, parts = source.get('slot'), source.get('kind'), source.get('parts')
        if type(slot) is not int or slot < 1 or slot in slots or kind not in ('current', 'history', 'workflow'):
            raise ValueError('Invalid context slot or kind')
        slots.add(slot)
        if not isinstance(parts, list) or not parts or any(
            not isinstance(part, dict) or not isinstance(part.get('text'), str) for part in parts
        ):
            raise ValueError('Invalid context parts')
        if kind != 'current':
            continue
        if 'repository' in source and source['repository'] != ref['repository']:
            raise ValueError('Context repository differs from pinned source')
        if 'branch' in source and source['branch'] not in (branch, f'refs/heads/{branch}'):
            raise ValueError('Context branch differs from pinned source')
        for part in parts:
            if part['revision'] != revision:
                raise ValueError('Context revision differs from pinned source')
            path = part['path']
            entry = git.files[revision][path]
            data = git.blob(entry['sha'])
            offsets = [0] + [m.end() for m in re.finditer(b'\n', data)]
            if offsets[-1] != len(data):
                offsets.append(len(data))
            first, last = part['startLine'], part['endLine']
            if type(first) is not int or type(last) is not int or not 1 <= first <= last < len(offsets):
                raise ValueError('Context line range is outside pinned source')
            lo, hi = offsets[first - 1], offsets[last]
            shown = part['text'].encode('utf-8')
            if not shown or shown.removesuffix(b'\n').count(b'\n') != last - first:
                raise ValueError('Context text does not cover stated line coordinates')
            matches = list(re.finditer(b'(?=' + re.escape(shown) + b')', data[lo:hi]))
            if len(matches) != 1:
                raise ValueError('Context text does not uniquely match pinned bytes on stated lines')
            start = lo + matches[0].start()
            ranges.setdefault((path, entry['sha']), []).append((start, start + len(shown)))
    return ranges


def model_context_coverage(case, row, git, ref, branch):
    if row.get('problem') is not None:
        return {'status': 'request_failure'}
    answer = row.get('answer')
    if not isinstance(answer, dict):
        return {'status': 'unavailable', 'reason': 'missing_or_malformed_answer'}
    context = answer.get('modelContext')
    if context is None:
        return {'status': 'unavailable', 'reason': 'missing_context'}
    try:
        if not isinstance(context, dict) or not isinstance(context.get('sources'), list):
            raise ValueError('Malformed model context')
        question = case['query']
        if not isinstance(question, str) or context.get('question') != question:
            raise ValueError('Context question differs from frozen question')
        if 'id' in case and row.get('id') != case['id']:
            raise ValueError('Response case ID differs from frozen case')
        request = row.get('request')
        if request is not None:
            if request['query'] != question:
                raise ValueError('Request query differs from frozen question')
            if 'requestId' in row and request.get('requestId') != row['requestId']:
                raise ValueError('Response request ID differs from submitted request')
        ranges = _context_ranges(context['sources'], git, ref, branch)
    except (KeyError, TypeError, ValueError) as error:
        return {'status': 'unavailable', 'reason': str(error)}
    facts = _fact_coverage(case, ranges, git, ref['commit']) if case.get('kind') == 'positive' else []
    return {'status': 'anchors_covered' if facts and all(fact['covered'] for fact in facts) else 'verified',
            'facts': facts, 'sources': len(context['sources']),
            'meaning': 'Verified API-returned context only, independent of publication and correctness.'}


def replay_response(case, row, git, ref, branch):
    from run_development import reopen, unavailable_control_outcome

    answer = row.get('answer')
    checked = (reopen(answer, git, ref, branch)
               if isinstance(answer, dict) and answer.get('outcome') == 'answered'
               and isinstance(answer.get('citations'), list) else [])
    report = {'id': row['id'], 'coverage': coverage(case, checked, git, ref['commit'], answer),
              'modelContextCoverage': model_context_coverage(case, row, git, ref, branch)}
    control_outcome = unavailable_control_outcome(case, answer, row.get('problem'))
    if control_outcome is not None:
        report['unavailableControlOutcome'] = control_outcome
    return report


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--manifest', type=Path, required=True)
    parser.add_argument('--repository', required=True)
    parser.add_argument('--git', type=Path, required=True)
    parser.add_argument('--responses', type=Path, required=True, help='Runner responses.ndjson with citationsReopened')
    args = parser.parse_args()
    manifest = read_manifest(args.manifest)
    verify(manifest, {args.repository: args.git}, args.repository, manifest_path=args.manifest)
    ref = json.loads((reference_path(manifest, args.repository, args.manifest)).read_text())
    git = inspect_git_tree(args.git, ref)
    cases = {c['id']: c for c in manifest['cases'] if c['repository'] == args.repository}
    seen = set()
    branch = manifest['fixtureBranches'][args.repository]
    for line in args.responses.read_text(encoding='utf-8').splitlines():
        row = json.loads(line)
        identifier = row['id']
        if identifier not in cases or identifier in seen:
            raise ValueError(f'Unknown or duplicate response: {identifier}')
        seen.add(identifier)
        print(json.dumps(replay_response(cases[identifier], row, git, ref, branch), ensure_ascii=False))
    if not seen:
        raise ValueError('Empty response file')


if __name__ == '__main__':
    main()
