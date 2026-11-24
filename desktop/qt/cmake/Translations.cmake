find_package(Python3 REQUIRED COMPONENTS Interpreter)

set(ctxfab_translation_dir "${CMAKE_CURRENT_BINARY_DIR}/i18n")
file(MAKE_DIRECTORY "${ctxfab_translation_dir}")
set(translation_languages fr de es it pt ja zh-Hans)
foreach(language IN LISTS translation_languages)
    list(APPEND ctxfab_qm_files "${ctxfab_translation_dir}/ctxfab_${language}.qm")
endforeach()

file(GLOB_RECURSE translation_sources CONFIGURE_DEPENDS
    "${CMAKE_CURRENT_SOURCE_DIR}/src/*.cpp"
    "${CMAKE_CURRENT_SOURCE_DIR}/src/*.h"
    "${CMAKE_CURRENT_SOURCE_DIR}/src/*.qml")
set(native_catalog "${CMAKE_CURRENT_SOURCE_DIR}/../macos/App/Localizable.xcstrings")
set(qt_catalog "${CMAKE_CURRENT_SOURCE_DIR}/i18n/qt-only.json")
set(translation_builder "${CMAKE_CURRENT_SOURCE_DIR}/i18n/build_catalogs.py")
set(inventory "${ctxfab_translation_dir}/sources.ts")

add_custom_command(OUTPUT ${ctxfab_qm_files}
    COMMAND "${CMAKE_COMMAND}" -E rm -f "${inventory}"
    COMMAND Qt6::lupdate -silent -source-language en -no-obsolete -locations none
        ${translation_sources} -ts "${inventory}"
    COMMAND Python3::Interpreter "${translation_builder}"
        "${inventory}" "${native_catalog}" "${qt_catalog}" "$<TARGET_FILE:Qt6::lrelease>"
        ${translation_languages}
    DEPENDS ${translation_sources} "${translation_builder}" "${native_catalog}" "${qt_catalog}"
        Qt6::lupdate Qt6::lrelease
    BYPRODUCTS "${inventory}"
    VERBATIM)
add_custom_target(ctxfab_translations DEPENDS ${ctxfab_qm_files})

function(ctxfab_add_translations target)
    add_dependencies("${target}" ctxfab_translations)
    qt_add_resources("${target}" "${target}_translations"
        PREFIX "/i18n" BASE "${ctxfab_translation_dir}" FILES ${ctxfab_qm_files})
endfunction()
