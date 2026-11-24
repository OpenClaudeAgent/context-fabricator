file(GLOB_RECURSE cpp_files "${SOURCE_DIR}/src/*.cpp" "${SOURCE_DIR}/src/*.h"
    "${SOURCE_DIR}/tests/*.cpp" "${SOURCE_DIR}/tests/*.h")
file(GLOB_RECURSE qml_files "${SOURCE_DIR}/src/*.qml" "${SOURCE_DIR}/tests/*.qml")

execute_process(
    COMMAND "${CLANG_FORMAT}" --dry-run --Werror ${cpp_files}
    COMMAND_ERROR_IS_FATAL ANY)
foreach(source IN LISTS qml_files)
    execute_process(
        COMMAND "${QML_FORMAT}" --ignore-settings --newline unix "${source}"
        OUTPUT_VARIABLE formatted
        COMMAND_ERROR_IS_FATAL ANY)
    file(READ "${source}" original)
    if(NOT original STREQUAL formatted)
        message(FATAL_ERROR "Run qmlformat --ignore-settings --newline unix -i ${source}")
    endif()
endforeach()
