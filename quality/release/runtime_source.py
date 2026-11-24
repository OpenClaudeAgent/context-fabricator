"""Build and attribute the pinned upstream llama.cpp server for local HTTP."""

import hashlib
import json
import shutil
import subprocess
import tarfile
from pathlib import Path

COMMIT = "b1b770d90d94cad9e9ea5b993bdbc383a522fb28"
SOURCE_ARCHIVE_SHA256 = "ae8556670b17c25931cd39eb7c95617650eaf6091bbcff206750583ddde1acb4"
BUILD_FLAGS = {
    "CMAKE_BUILD_TYPE": "Release",
    "CMAKE_OSX_ARCHITECTURES": "arm64",
    "CMAKE_OSX_DEPLOYMENT_TARGET": "14.0",
    "CMAKE_BUILD_WITH_INSTALL_RPATH": "ON",
    "CMAKE_INSTALL_RPATH": "@loader_path",
    "BUILD_SHARED_LIBS": "ON",
    "LLAMA_BUILD_NUMBER": "12359",
    "LLAMA_BUILD_COMMIT": "b1b770d90",
    "LLAMA_BUILD_IS_DEV": "OFF",
    "LLAMA_BUILD_UI": "OFF",
    "LLAMA_USE_PREBUILT_UI": "OFF",
    "LLAMA_OPENSSL": "OFF",
    "LLAMA_CONNECT": "OFF",
    "LLAMA_BUILD_TESTS": "OFF",
    "LLAMA_BUILD_EXAMPLES": "OFF",
    "LLAMA_BUILD_APP": "OFF",
    "LLAMA_BUILD_SERVER": "ON",
    "GGML_NATIVE": "OFF",
    "GGML_METAL": "ON",
    "GGML_METAL_EMBED_LIBRARY": "ON",
    "GGML_RPC": "OFF",
}
LINUX_BUILD_FLAGS = {
    name: value for name, value in BUILD_FLAGS.items()
    if not name.startswith("CMAKE_OSX_") and not name.startswith("GGML_METAL")
    and name not in {"CMAKE_BUILD_WITH_INSTALL_RPATH", "CMAKE_INSTALL_RPATH"}
} | {
    "BUILD_SHARED_LIBS": "OFF",
    "CMAKE_C_FLAGS": '"-ffile-prefix-map=@BUILD_ROOT@=."',
    "CMAKE_CXX_FLAGS": '"-ffile-prefix-map=@BUILD_ROOT@=."',
    "GGML_METAL": "OFF",
    "GGML_OPENMP": "OFF",
    "GGML_BLAS": "OFF",
    "GGML_BACKEND_DL": "OFF",
    "GGML_CCACHE": "OFF",
    "GGML_SSE42": "OFF",
    "GGML_AVX": "OFF",
    "GGML_AVX2": "OFF",
    "GGML_BMI2": "OFF",
    "GGML_FMA": "OFF",
    "GGML_F16C": "OFF",
}
LICENSE_FILES = {
    "LICENSE-cpp-httplib.txt": "vendor/cpp-httplib/LICENSE",
    "LICENSE-nlohmann-json.txt": "licenses/LICENSE-jsonhpp",
    "LICENSE-sha256.txt": "vendor/hash/sha256/LICENSE",
    "LICENSE-rotate-bits.txt": "vendor/hash/rotate-bits/LICENSE.md",
}
HEADER_LICENSES = {
    "LICENSE-base64.txt": ("common/base64.hpp", "free and unencumbered software"),
    "LICENSE-miniaudio.txt": ("vendor/miniaudio/miniaudio.h", "ALTERNATIVE 1 - Public Domain"),
    "LICENSE-stb-image.txt": ("vendor/stb/stb_image.h", "ALTERNATIVE A - MIT License"),
    "LICENSE-subprocess.txt": ("vendor/sheredom/subprocess.h", "free and unencumbered software"),
}
SUPPLEMENTS = {
    "LICENSE-yarn.txt": ("jquesnelle/yarn", "995db5b575e75230b3384d658f8b944c9662f775"),
    "LICENSE-ggllm.txt": ("cmp-nct/ggllm.cpp", "0dc65eb735f90f82143ead9dedb66e73fa25d114"),
    "LICENSE-pillow.txt": ("python-pillow/Pillow", "693df7b42c666f88c719f9973be0ad71607328e0"),
    "LICENSE-pytorch.txt": ("pytorch/pytorch", "d38164a545b4a4e4e0cf73ce67173f70574890b6"),
    "LICENSE-arm-optimized-routines.txt": (
        "ARM-software/optimized-routines", "56e3bf05c19c4e28e1f5edd9093c712f16c5c32a"),
    "LICENSE-fp16.txt": ("Maratyszcza/FP16", "782eea126dc5c755827be751a099eb01826175cf"),
    "LICENSE-unicode.txt": (
        "unicode-org/unicodetools", "725fb6055cd42915c74bcf77b91aa8fb8ef18669"),
}
NOTICE_FILENAMES = set(LICENSE_FILES) | set(HEADER_LICENSES) | set(SUPPLEMENTS) | {
    "LICENSE-llamafile-sgemm.txt", "NOTICE-arm-optimized-routines.txt",
}


def digest(path):
    with path.open("rb") as contents:
        return hashlib.file_digest(contents, "sha256").hexdigest()


def build_server(archive, stage, *, flags=BUILD_FLAGS):
    if digest(archive) != SOURCE_ARCHIVE_SHA256:
        raise ValueError("The source archive does not match the pinned upstream commit.")
    with tarfile.open(archive) as package:
        package.extractall(stage, filter="data")
    source = stage / f"llama.cpp-{COMMIT}"
    build = stage / "build"
    definitions = [f"-D{name}={value.replace('@BUILD_ROOT@', str(stage.resolve()))}"
                   for name, value in flags.items()]
    subprocess.run(["cmake", "-S", source, "-B", build, "-G", "Ninja", *definitions], check=True)
    subprocess.run(["cmake", "--build", build, "--target", "llama-server", "--parallel", "4"], check=True)
    binaries = build / "bin"
    shutil.copyfile(source / "LICENSE", binaries / "LICENSE")
    return source, binaries


def header_license(path, anchor):
    text = path.read_text(encoding="utf-8")
    center = text.index(anchor)
    start = text.rindex("/*", 0, center)
    end = text.index("*/", center)
    return text[start + 2:end].strip() + "\n"


def build_tool_versions(build):
    cache = (build / "CMakeCache.txt").read_text(encoding="utf-8").splitlines()
    tools = {"cmake": "cmake", "ninja": "ninja"}
    for language in ("C", "CXX"):
        [compiler] = [line.split("=", 1)[1] for line in cache
                      if line.startswith(f"CMAKE_{language}_COMPILER:")]
        tools[language] = compiler
    return {name: subprocess.check_output([program, "--version"], text=True).splitlines()[0]
            for name, program in tools.items()}


def stage_notices(source, build, destination, *, flags=BUILD_FLAGS):
    notices = destination / "ThirdPartyNotices"
    notices.mkdir()
    for filename, path in LICENSE_FILES.items():
        shutil.copyfile(source / path, notices / filename)
    for filename, (path, anchor) in HEADER_LICENSES.items():
        (notices / filename).write_text(header_license(source / path, anchor), encoding="utf-8")
    sgemm = (source / "ggml/src/ggml-cpu/llamafile/sgemm.cpp").read_text(encoding="utf-8")
    license_header, _, _ = sgemm.partition("// SOFTWARE.\n")
    lines = (license_header + "// SOFTWARE.").splitlines()
    if not license_header.startswith("// Copyright 2024 Mozilla Foundation") or not all(
            line.startswith("//") for line in lines):
        raise ValueError("Cannot identify the llamafile SGEMM license header.")
    (notices / "LICENSE-llamafile-sgemm.txt").write_text(
        "\n".join(line[2:].lstrip() for line in lines) + "\n", encoding="utf-8")
    (notices / "NOTICE-arm-optimized-routines.txt").write_text(
        "Arm vector exponential coefficients in ggml are adapted from math/v_expf.c.\n"
        "Copyright (c) 2019-2022, Arm Limited.\n"
        "See LICENSE-arm-optimized-routines.txt for the upstream license options.\n", encoding="utf-8")
    supplements = Path(__file__).with_name("notices") / "runtime"
    for filename in SUPPLEMENTS:
        shutil.copyfile(supplements / filename, notices / filename)
    assets = {filename: digest(notices / filename) for filename in sorted(NOTICE_FILENAMES)}
    provenance = {
        "sourceCommit": COMMIT,
        "sourceArchiveSHA256": SOURCE_ARCHIVE_SHA256,
        "cmake": flags,
        "tools": build_tool_versions(build),
        "supplementSources": {name: f"https://github.com/{repository}/blob/{revision}/LICENSE"
                              for name, (repository, revision) in SUPPLEMENTS.items()},
        "upstreamLicenseSHA256": digest(destination / "LICENSE-llama.cpp"),
        "notices": assets,
    }
    (destination / "runtime-source.json").write_text(json.dumps(provenance, indent=2) + "\n",
                                                       encoding="utf-8")


def verify_notices(bundle):
    verify_runtime_notices(bundle / "Contents/Resources", BUILD_FLAGS)


def verify_runtime_notices(resources, flags):
    provenance = resources / "runtime-source.json"
    if not provenance.is_file() or provenance.is_symlink():
        raise ValueError("Managed runtime packaging requires pinned API-source provenance.")
    record = json.loads(provenance.read_text(encoding="utf-8"))
    if (record["sourceCommit"] != COMMIT or record["sourceArchiveSHA256"] != SOURCE_ARCHIVE_SHA256
            or record["cmake"] != flags or set(record["notices"]) != NOTICE_FILENAMES):
        raise ValueError("Unexpected API runtime source or notice inventory.")
    if digest(resources / "LICENSE-llama.cpp") != record["upstreamLicenseSHA256"]:
        raise ValueError("The upstream runtime license does not match its manifest.")
    notices = resources / "ThirdPartyNotices"
    if {file.name for file in notices.iterdir()} != NOTICE_FILENAMES:
        raise ValueError("Missing or unexpected runtime notice files.")
    for filename, checksum in record["notices"].items():
        file = notices / filename
        if file.is_symlink() or digest(file) != checksum:
            raise ValueError(f"Runtime notice checksum mismatch: {filename}.")


if __name__ == "__main__":
    import sys

    verify_notices(Path(sys.argv[1]))
