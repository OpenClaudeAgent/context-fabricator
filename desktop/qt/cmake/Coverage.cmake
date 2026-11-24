if(NOT BUILD_TESTING OR NOT CMAKE_CXX_COMPILER_ID MATCHES "^(AppleClang|Clang)$")
    message(FATAL_ERROR "Qt coverage requires Clang and BUILD_TESTING")
endif()

if(CMAKE_CXX_COMPILER_ID STREQUAL "AppleClang")
    execute_process(COMMAND xcrun --find llvm-profdata
        OUTPUT_VARIABLE llvm_profdata OUTPUT_STRIP_TRAILING_WHITESPACE COMMAND_ERROR_IS_FATAL ANY)
    execute_process(COMMAND xcrun --find llvm-cov
        OUTPUT_VARIABLE llvm_cov OUTPUT_STRIP_TRAILING_WHITESPACE COMMAND_ERROR_IS_FATAL ANY)
else()
    string(REGEX MATCH "^[0-9]+" clang_major "${CMAKE_CXX_COMPILER_VERSION}")
    get_filename_component(clang_bin "${CMAKE_CXX_COMPILER}" DIRECTORY)
    find_program(llvm_profdata NAMES "llvm-profdata-${clang_major}" llvm-profdata
        HINTS "${clang_bin}" REQUIRED)
    find_program(llvm_cov NAMES "llvm-cov-${clang_major}" llvm-cov HINTS "${clang_bin}" REQUIRED)
endif()

set(profile_sources "${CMAKE_CURRENT_LIST_DIR}/CoverageSources.txt")
set_property(DIRECTORY APPEND PROPERTY CMAKE_CONFIGURE_DEPENDS "${profile_sources}")
target_compile_options(ctxfab_qt_options INTERFACE
    -fprofile-instr-generate -fcoverage-mapping "-fprofile-list=${profile_sources}")
target_link_options(ctxfab_qt_options INTERFACE -fprofile-instr-generate)

set(coverage_binaries ctxfab_qt ctxfab_qt_window_tests ctxfab_app_language_tests ctxfab_session_tests
    ctxfab_overview_tests ctxfab_preferences_tests ctxfab_github_tests ctxfab_trace_tests
    ctxfab_answers_tests ctxfab_models_tests ctxfab_inference_tests ctxfab_pipeline_tests)
set(coverage_objects "")
foreach(binary IN LISTS coverage_binaries)
    list(APPEND coverage_objects "$<TARGET_FILE:${binary}>")
endforeach()

add_custom_target(ctxfab-coverage
    COMMAND Python3::Interpreter -B "${CMAKE_CURRENT_SOURCE_DIR}/tests/coverage.py"
        "${CMAKE_CTEST_COMMAND}" "${CMAKE_CURRENT_BINARY_DIR}" "${CMAKE_CURRENT_SOURCE_DIR}/src"
        "${llvm_profdata}" "${llvm_cov}" ${coverage_objects}
    DEPENDS ${coverage_binaries}
    USES_TERMINAL VERBATIM)
