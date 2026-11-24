# Local build output

Use temporary workspaces for one-off builds, model fixtures and profiling. Shell scripts under `quality/` can use `quality_workspace` from `quality/toolchain/environment.sh`. Clean the workspace when the command exits.

Keep local `.quality/` for managed qualification reports within the 2 GB budget. Reuse the normal Xcode, CMake and Cargo build locations. Keep requested screenshots and compact proof under `.artifacts/evidence/`.

Use the fixed headless build directory from `quality/macos/app.sh` when the Xcode GUI is building. Concurrent builds in the IDE's DerivedData contend for the same build database.
