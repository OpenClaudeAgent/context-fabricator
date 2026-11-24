qt_add_executable(ctxfab_qt MACOSX_BUNDLE main.cpp)
target_link_libraries(ctxfab_qt PRIVATE ctxfab_qt_options ctxfab_qt_ui ctxfab_qt_uiplugin)
set_target_properties(ctxfab_qt PROPERTIES
    MACOSX_BUNDLE_GUI_IDENTIFIER org.contextfabricator.qt
    RUNTIME_OUTPUT_DIRECTORY "${PROJECT_BINARY_DIR}")

if(CTXFAB_LINUX_DEPLOYMENT)
    if(NOT TARGET Qt6::QWaylandIntegrationPlugin)
        message(FATAL_ERROR "Install the Qt Wayland module for Linux packaging.")
    endif()
    set_target_properties(ctxfab_qt PROPERTIES
        QT_QML_IMPORT_SCANNER_EXTRA_ROOT_PATHS "${PROJECT_SOURCE_DIR}/src"
        INSTALL_RPATH "$ORIGIN/../${CMAKE_INSTALL_LIBDIR}")
    install(TARGETS ctxfab_qt RUNTIME DESTINATION "${CMAKE_INSTALL_BINDIR}")
    qt6_generate_deploy_qml_app_script(TARGET ctxfab_qt OUTPUT_SCRIPT qt_deployment
        INCLUDE_PLUGINS qxcb qwayland EXCLUDE_PLUGINS qgtk3 adwaita
        EXCLUDE_PLUGIN_TYPES egldeviceintegrations NO_TRANSLATIONS)
    install(SCRIPT "${qt_deployment}")
endif()
