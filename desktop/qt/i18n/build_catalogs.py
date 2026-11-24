"""Build complete Qt catalogs from extracted messages and the shared product copy."""

import json
import re
import subprocess
import sys
import xml.etree.ElementTree as ET
from pathlib import Path


SWIFT_FORMATS = {
    "%1 · Recommended · %2": "%@ · Recommended · %@",
    "After · %1 @ %2": "After · %@ @ %@",
    "Before · %1 @ %2": "Before · %@ @ %@",
    "Line %1": "Line %lld",
    "Lines %1–%2": "Lines %lld–%lld",
    "Proposed input %1": "Proposed input %lld",
    "Repository %1 · %2": "Repository %@ · %@",
    "Search rank %1": "Search rank %lld",
    "%1 of %2 history pairs selected": "%lld of %lld history pairs selected",
    "%1 of %2 input tokens": "%@ of %@ input tokens",
    "%1 of %2 linked passages selected": "%lld of %lld linked passages selected",
    "%1 of %2 passages selected": "%lld of %lld passages selected",
    "%1 of %2 workflow attempts selected": "%lld of %lld workflow attempts selected",
    "Dense #%1": "Dense #%lld",
    "Fusion #%1": "Fusion #%lld",
    "Lexical #%1": "Lexical #%lld",
    "No evidence fit within the %1-token limit.": "No evidence fit within the %@-token limit.",
    "Score #%1": "Score #%lld",
    "Score %1": "Score %@",
    "%1 / %2 passages": "%lld / %lld passages",
    "%1 GB (%2 bytes)": "%@ GB (%@ bytes)",
    "Actions run: %1": "Actions run: %lld",
    "%1 of %2": "%@ of %@",
    "Build %1": "Build %@",
    "Removed %1 vectors (%2 of logical data). Disk space reclaimed may differ.":
        "Removed %lld vectors (%@ of logical data). Disk space reclaimed may differ.",
    "Runtime archive: %1": "Runtime archive: %@",
    "%1Delete the local weights for %2 (%3 of logical data)? You can download them again.":
        "%@Delete the local weights for %@ (%@ of logical data)? You can download them again.",
    "Discard %1 of partial data for %2? Installed weights are retained.":
        "Discard %@ of partial data for %@? Installed weights are retained.",
    "Download failed · %1 of %2": "Download failed · %@ of %@",
    "Downloading · %1 of %2": "Downloading · %@ of %@",
    "Inference storage: %1 used of %2": "Inference storage: %@ used of %@",
    "Installed · %1": "Installed · %@",
    "Interrupted · %1 of %2": "Interrupted · %@ of %@",
    "Not installed · %1": "Not installed · %@",
    "Paused · %1 of %2": "Paused · %@ of %@",
    "Pausing · %1 of %2": "Pausing · %@ of %@",
    "Reserved: %1 · Available: %2": "Reserved: %@ · Available: %@",
    "Attempt: %1": "Attempt: %lld",
    "Branch: %1": "Branch: %@",
    "Commit: %1": "Commit: %@",
    "Choose 1–%1 requests per minute for all repositories.":
        "Choose 1–%lld requests per minute for all repositories.",
    "%1 reachable files · %2 parse errors · %3 limited":
        "%lld reachable files · %lld parse errors · %lld limited",
    "Results for %1 in %2": "Results for %@ in %@",
    "Repository %1": "Repository %lld",
    "Open run %1, attempt %2 on GitHub": "Open run %lld, attempt %lld on GitHub",
    "Run %1 · attempt %2 · %3 · %4": "Run %lld · attempt %lld · %@ · %@",
    "Run %1 · attempt %2 · reported head %3": "Run %lld · attempt %lld · reported head %@",
    "%1 observed": "%lld observed",
    "Workflow #%1": "Workflow #%lld",
}

SWIFT_NUMERUS = {
    "%n candidate": "%lld candidates",
    "%n candidates": "%lld candidates",
}


def native_translation(native, source, language):
    if source in SWIFT_FORMATS:
        unit = native[SWIFT_FORMATS[source]]["localizations"][language]["stringUnit"]
        positions = iter(re.findall(r"%[1-9][0-9]*", source))
        return re.sub(
            r"%(?:([1-9][0-9]*)\$)?(?:lld|@)",
            lambda match: f"%{match[1]}" if match[1] else next(positions),
            unit["value"],
        )
    return native.get(source, {}).get("localizations", {}).get(language, {}).get(
        "stringUnit", {}
    ).get("value")


def plural_forms(native, source, language):
    native_key = SWIFT_NUMERUS.get(source)
    localization = native.get(native_key, {}).get("localizations", {}).get(language, {})
    entries = localization.get("variations", {}).get("plural", {})
    return [
        entries[category]["stringUnit"]["value"].replace("%lld", "%n")
        for category in ("one", "other")
        if category in entries
    ]


def validate_placeholders(source, translation, language):
    placeholders = r"%(?:L?[1-9][0-9]*|n)"
    if sorted(re.findall(placeholders, source)) != sorted(re.findall(placeholders, translation)):
        raise SystemExit(f"{language}: mismatched placeholders for {source!r}")


def compile_catalog(inventory, native, extra, lrelease, language):
    tree = ET.parse(inventory)
    tree.getroot().set("language", language.replace("-", "_"))
    missing = set()
    for message in tree.iter("message"):
        source = message.findtext("source")
        translated = message.find("translation")
        if message.get("numerus") == "yes":
            forms = plural_forms(native, source, language)
            if not forms:
                missing.add(source)
                continue
            translated.clear()
            for form in forms:
                validate_placeholders(source, form, language)
                ET.SubElement(translated, "numerusform").text = form
            continue
        translation = extra.get(source, {}).get(language)
        if translation is None:
            translation = native_translation(native, source, language)
        if not translation:
            missing.add(source)
            continue
        validate_placeholders(source, translation, language)
        translated.attrib.pop("type", None)
        translated.text = translation
    if missing:
        messages = "\n".join(f"  {source}" for source in sorted(missing))
        raise SystemExit(f"Missing {language} translations:\n{messages}")

    ts_file = inventory.with_name(f"ctxfab_{language}.ts")
    qm_file = ts_file.with_suffix(".qm")
    tree.write(ts_file, encoding="utf-8", xml_declaration=True)
    subprocess.run(
        [lrelease, "-silent", "-fail-on-unfinished", str(ts_file), "-qm", str(qm_file)],
        check=True,
    )


def main():
    inventory, native_file, extra_file, lrelease, *languages = sys.argv[1:]
    native = json.loads(Path(native_file).read_text(encoding="utf-8"))["strings"]
    extra = json.loads(Path(extra_file).read_text(encoding="utf-8"))
    for language in languages:
        compile_catalog(Path(inventory), native, extra, lrelease, language)


if __name__ == "__main__":
    main()
