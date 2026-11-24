cmake_minimum_required(VERSION 3.25)
project(ContextFabricatorQt LANGUAGES CXX)

set(CMAKE_CXX_STANDARD 20)
set(CMAKE_CXX_STANDARD_REQUIRED ON)
set(CMAKE_CXX_EXTENSIONS OFF)

include(CTest)
option(CTXFAB_QUALITY_TOOLS "Require the static and formatting tools" OFF)
option(CTXFAB_SANITIZERS "Enable AddressSanitizer and UndefinedBehaviorSanitizer" OFF)
option(CTXFAB_COVERAGE "Instrument the first-party C++ and enable the coverage gate" OFF)
option(CTXFAB_OPENAPI_UNITY "Compile generated OpenAPI models in small batches" OFF)
option(CTXFAB_LINUX_DEPLOYMENT "Install a standalone Linux Qt application tree" OFF)

if(CTXFAB_LINUX_DEPLOYMENT)
    if(NOT CMAKE_HOST_SYSTEM_NAME STREQUAL "Linux" OR NOT CMAKE_SYSTEM_NAME STREQUAL "Linux"
       OR CMAKE_CROSSCOMPILING)
        message(FATAL_ERROR "Qt Linux deployment requires a native Linux build.")
    endif()
    include(GNUInstallDirs)
endif()

find_package(Qt6 6.12 REQUIRED COMPONENTS Core Gui LinguistTools Network Quick QuickControls2)
qt_standard_project_setup(REQUIRES 6.12)

include(cmake/OpenApi.cmake)
ctxfab_generate_openapi("${CMAKE_CURRENT_SOURCE_DIR}/../../contracts/openapi.yaml")
include(cmake/Translations.cmake)

if(CTXFAB_QUALITY_TOOLS)
    find_program(CTXFAB_TIDY NAMES clang-tidy-19 clang-tidy REQUIRED)
    set(CMAKE_CXX_CLANG_TIDY "${CTXFAB_TIDY};--warnings-as-errors=*")
endif()

add_library(ctxfab_qt_options INTERFACE)
target_compile_options(ctxfab_qt_options INTERFACE -Wall -Wextra -Wpedantic -Werror)
if(CTXFAB_SANITIZERS)
    set(sanitizers -fsanitize=address,undefined -fno-omit-frame-pointer)
    target_compile_options(ctxfab_qt_options INTERFACE ${sanitizers})
    target_link_options(ctxfab_qt_options INTERFACE ${sanitizers})
    target_compile_options(ctxfab_openapi PRIVATE ${sanitizers})
endif()

set(app_directory src/app)
set(platform_directory src/platform)
set(ui_directory src/ui)
set(overview_directory src/features/overview)
set(settings_directory src/features/settings)
set(github_directory src/features/github)
set(diagnostics_directory src/features/diagnostics)
set(answers_directory src/features/answers)
set(models_directory src/features/models)
set(pipeline_directory src/features/pipeline)

add_library(ctxfab_session STATIC
    ${platform_directory}/BackendSupervisor.cpp ${platform_directory}/BackendSupervisor.h
    ${platform_directory}/Session.cpp ${platform_directory}/Session.h)
target_include_directories(ctxfab_session PUBLIC ${platform_directory})
target_link_libraries(ctxfab_session PRIVATE ctxfab_qt_options PUBLIC Qt6::Core)

add_library(ctxfab_local_api STATIC
    ${platform_directory}/LocalApiRequest.cpp ${platform_directory}/LocalApiRequest.h)
target_link_libraries(ctxfab_local_api PRIVATE ctxfab_qt_options PUBLIC ctxfab_openapi ctxfab_session)

set(app_sources ${app_directory}/AppLanguage.cpp ${app_directory}/AppLanguage.h)
set(overview_sources ${overview_directory}/Overview.cpp ${overview_directory}/Overview.h)
set(answers_sources
    ${answers_directory}/Answers.cpp ${answers_directory}/Answers.h
    ${answers_directory}/AnswerHistory.cpp ${answers_directory}/AnswerHistory.h
    ${answers_directory}/SavedAnswer.cpp ${answers_directory}/SavedAnswer.h
    ${answers_directory}/AnswerResult.cpp ${answers_directory}/AnswerResult.h)
set(preference_sources
    ${settings_directory}/Preferences.cpp ${settings_directory}/Preferences.h
    ${settings_directory}/HistoryPreferences.cpp ${settings_directory}/HistoryPreferences.h)
set(model_sources
    ${models_directory}/ModelLibrary.cpp ${models_directory}/ModelLibrary.h
    ${models_directory}/HubCatalog.cpp ${models_directory}/HubCatalog.h
    ${models_directory}/LibraryModel.cpp ${models_directory}/LibraryModel.h
    ${models_directory}/LibraryEntries.cpp ${models_directory}/LibraryEntries.h)
set(inference_control_sources
    ${models_directory}/InferenceSetup.cpp ${models_directory}/InferenceSetup.h
    ${models_directory}/LocalRuntime.cpp ${models_directory}/LocalRuntime.h
    ${models_directory}/EmbeddingIndex.cpp ${models_directory}/EmbeddingIndex.h
    ${models_directory}/EmbeddingProfiles.cpp ${models_directory}/EmbeddingProfiles.h)
set(github_sources
    ${github_directory}/GitHubConnector.cpp ${github_directory}/GitHubConnector.h
    ${github_directory}/GitHubRepositoryEntry.cpp ${github_directory}/GitHubRepositoryEntry.h
    ${github_directory}/GitHubRepositoryList.cpp ${github_directory}/GitHubRepositoryList.h
    ${github_directory}/GitHubActionsActivity.cpp ${github_directory}/GitHubActionsActivity.h
    ${github_directory}/GitHubActionsRun.cpp ${github_directory}/GitHubActionsRun.h
    ${github_directory}/GitHubCollection.cpp ${github_directory}/GitHubCollection.h
    ${github_directory}/GitHubCollectionTraceLink.cpp ${github_directory}/GitHubCollectionTraceLink.h
    ${github_directory}/GitHubGraphBrowser.cpp ${github_directory}/GitHubGraphBrowser.h
    ${github_directory}/GitHubGraphOccurrence.cpp ${github_directory}/GitHubGraphOccurrence.h
    ${github_directory}/GitHubUsage.cpp ${github_directory}/GitHubUsage.h)
set(trace_record_sources
    ${diagnostics_directory}/RecordedAnswerDetails.cpp ${diagnostics_directory}/RecordedAnswerDetails.h
    ${diagnostics_directory}/TraceRecord.cpp ${diagnostics_directory}/TraceRecord.h)
set(diagnostics_sources
    ${diagnostics_directory}/TraceInspector.cpp ${diagnostics_directory}/TraceInspector.h
    ${trace_record_sources})
set(trace_answer_sources
    ${diagnostics_directory}/TraceAnswer.cpp ${diagnostics_directory}/TraceAnswer.h
    ${diagnostics_directory}/TraceAnswerDetails.cpp ${diagnostics_directory}/TraceAnswerDetails.h)
set(diagnostic_log_sources
    ${diagnostics_directory}/DiagnosticLogs.cpp ${diagnostics_directory}/DiagnosticLogs.h
    ${diagnostics_directory}/DiagnosticEvent.h)
set(answer_request_sources
    ${diagnostics_directory}/DiagnosticAnswerRequests.cpp ${diagnostics_directory}/DiagnosticAnswerRequests.h)
set(telemetry_sources
    ${diagnostics_directory}/Diagnostics.cpp ${diagnostics_directory}/Diagnostics.h
    ${diagnostics_directory}/AnswerUsageModel.cpp ${diagnostics_directory}/AnswerUsageModel.h
    ${diagnostic_log_sources})
set(pipeline_sources
    ${pipeline_directory}/BenchmarkCatalog.cpp ${pipeline_directory}/BenchmarkCatalog.h
    ${pipeline_directory}/BenchmarkSources.cpp ${pipeline_directory}/BenchmarkSources.h
    ${pipeline_directory}/PipelineInspector.cpp ${pipeline_directory}/PipelineInspector.h
    ${pipeline_directory}/PipelineCorpusImport.cpp ${pipeline_directory}/PipelineConnectedSources.cpp)

set(qml_sources
    ${app_directory}/Main.qml ${app_directory}/NavButton.qml ${app_directory}/NavIcon.qml
    ${ui_directory}/Theme.qml ${ui_directory}/ActionButton.qml
    ${ui_directory}/SurfaceGroup.qml ${ui_directory}/SurfaceDialog.qml ${ui_directory}/SurfaceDivider.qml
    ${ui_directory}/DialogActions.qml ${ui_directory}/ConfirmationDialog.qml
    ${ui_directory}/PageTitle.qml ${ui_directory}/StatusChip.qml ${ui_directory}/SurfaceInput.qml
    ${ui_directory}/SegmentTab.qml
    ${ui_directory}/MetricRow.qml ${ui_directory}/LocalizedComboBox.qml
    ${overview_directory}/OverviewPage.qml
    ${answers_directory}/AnswersPage.qml ${answers_directory}/AnswerComposer.qml
    ${answers_directory}/AnswerResultPanel.qml
    ${answers_directory}/AnswerHistoryDialog.qml ${answers_directory}/SavedAnswerDetail.qml
    ${models_directory}/ModelLibraryDialog.qml ${models_directory}/LibraryModelSection.qml
    ${models_directory}/HubCatalogPage.qml
    ${models_directory}/InferenceSetupDialog.qml ${models_directory}/InferenceSetupReview.qml
    ${models_directory}/SetupModelSource.qml
    ${models_directory}/LibraryModelRow.qml ${models_directory}/LibrarySection.qml
    ${models_directory}/LocalRuntimeSection.qml ${models_directory}/EmbeddingIndexSection.qml
    ${models_directory}/EmbeddingProfileSetting.qml
    ${settings_directory}/SettingsPage.qml ${settings_directory}/SettingsInput.qml
    ${settings_directory}/HistorySettingsGroup.qml
    ${github_directory}/ConnectorsPage.qml ${github_directory}/GitHubPage.qml
    ${github_directory}/GitHubRepositoryDialog.qml ${github_directory}/GitHubRepositoryPicker.qml
    ${github_directory}/GitHubConfiguredRepositoryRow.qml ${github_directory}/GitHubRepositoryVisibility.qml
    ${github_directory}/GitHubAccountForm.qml ${github_directory}/GitHubActionsDialog.qml
    ${github_directory}/GitHubActionsRunRow.qml
    ${github_directory}/GitHubGraphDialog.qml ${github_directory}/GitHubGraphResult.qml
    ${github_directory}/GitHubCollectionTraceLinks.qml
    ${diagnostics_directory}/TraceInspectorDialog.qml ${diagnostics_directory}/TraceOutlinePane.qml
    ${diagnostics_directory}/TraceAnswerPane.qml ${diagnostics_directory}/TraceEvidencePanel.qml
    ${diagnostics_directory}/TraceModelSources.qml ${diagnostics_directory}/TraceExcerptView.qml
    ${diagnostics_directory}/TraceSpanDetail.qml ${diagnostics_directory}/TraceLogRow.qml
    ${diagnostics_directory}/TraceDetailsGroup.qml
    ${diagnostics_directory}/DiagnosticsPage.qml ${diagnostics_directory}/DiagnosticsSignalList.qml
    ${diagnostics_directory}/DiagnosticsStorage.qml ${diagnostics_directory}/DiagnosticLogsHeader.qml
    ${diagnostics_directory}/DiagnosticSignalRow.qml
    ${diagnostics_directory}/DiagnosticRequestsHeader.qml ${diagnostics_directory}/DiagnosticRequestRow.qml
    ${diagnostics_directory}/AnswerUsageHeader.qml ${diagnostics_directory}/AnswerUsageCard.qml
    ${pipeline_directory}/PipelineInspectorDialog.qml ${pipeline_directory}/PipelineBenchmarkPane.qml
    ${pipeline_directory}/PipelineDetailPane.qml ${pipeline_directory}/PipelineSourcePane.qml
    ${github_directory}/GitHubCollectionSection.qml ${github_directory}/GitHubUsageDialog.qml)
foreach(qml_source IN LISTS qml_sources)
    get_filename_component(resource_name "${qml_source}" NAME)
    set_source_files_properties("${qml_source}" PROPERTIES QT_RESOURCE_ALIAS "${resource_name}")
endforeach()
set_source_files_properties(${ui_directory}/Theme.qml PROPERTIES QT_QML_SINGLETON_TYPE TRUE)
qt_add_qml_module(ctxfab_qt_ui STATIC
    URI ContextFabricator
    CLASS_NAME ContextFabricatorPlugin
    OUTPUT_DIRECTORY "${CMAKE_CURRENT_BINARY_DIR}/ContextFabricator"
    SOURCES ${app_sources} ${overview_sources} ${answers_sources} ${preference_sources} ${github_sources}
        ${diagnostics_sources} ${telemetry_sources} ${trace_answer_sources} ${answer_request_sources} ${model_sources}
        ${inference_control_sources} ${pipeline_sources}
    QML_FILES ${qml_sources})
target_include_directories(ctxfab_qt_ui PUBLIC
    ${app_directory} ${overview_directory} ${answers_directory} ${settings_directory} ${github_directory}
    ${diagnostics_directory} ${models_directory} ${pipeline_directory})
target_link_libraries(ctxfab_qt_ui
    PRIVATE ctxfab_qt_options
    PUBLIC ctxfab_local_api Qt6::Quick Qt6::QuickControls2)
ctxfab_add_translations(ctxfab_qt_ui)
set(benchmark_catalog_directory
    "${CMAKE_CURRENT_SOURCE_DIR}/../macos/App/Features/Diagnostics/Resources")
qt_add_resources(ctxfab_qt_ui ctxfab_benchmark_catalog
    PREFIX "/ctxfab/benchmarks" BASE "${benchmark_catalog_directory}"
    FILES "${benchmark_catalog_directory}/PipelineBenchmarks.json"
        "${benchmark_catalog_directory}/PipelineBenchmarkSuites.json")

add_subdirectory(${app_directory})
add_custom_target(ctxfab_qt_qmllint DEPENDS ctxfab_qt_ui_qmllint)

if(BUILD_TESTING)
    find_package(Qt6 6.12 REQUIRED COMPONENTS Test)
    qt_add_executable(ctxfab_qt_window_tests ${app_directory}/main.cpp tests/WindowScenarios.cpp)
    set_target_properties(ctxfab_qt_window_tests PROPERTIES EXCLUDE_FROM_ALL ON)
    target_link_libraries(ctxfab_qt_window_tests PRIVATE
        ctxfab_qt_options ctxfab_qt_ui ctxfab_qt_uiplugin Qt6::Test)

    # Link the same implementation and resources as the app, once per build.
    add_executable(ctxfab_app_language_tests tests/AppLanguageTests.cpp)
    target_link_libraries(ctxfab_app_language_tests PRIVATE ctxfab_qt_options ctxfab_qt_ui Qt6::Test)
    add_test(NAME qt-app-language COMMAND ctxfab_app_language_tests)
    set_tests_properties(qt-app-language PROPERTIES TIMEOUT 10)

    add_test(NAME qt-qml-components COMMAND Qt6::qmltestrunner
        -input "${CMAKE_CURRENT_SOURCE_DIR}/tests/qml")
    set_tests_properties(qt-qml-components PROPERTIES TIMEOUT 10 ENVIRONMENT
        "QT_QPA_PLATFORM=offscreen;QT_QUICK_BACKEND=software;QT_QUICK_CONTROLS_STYLE=Fusion")

    add_executable(ctxfab_session_tests tests/SessionTests.cpp)
    target_link_libraries(ctxfab_session_tests PRIVATE ctxfab_qt_options ctxfab_session Qt6::Test)
    add_test(NAME qt-session COMMAND ctxfab_session_tests)
    set_tests_properties(qt-session PROPERTIES TIMEOUT 15)

    add_executable(ctxfab_overview_tests tests/OverviewTests.cpp)
    target_link_libraries(ctxfab_overview_tests PRIVATE
        ctxfab_qt_options ctxfab_qt_ui Qt6::Test)
    add_test(NAME qt-overview COMMAND ctxfab_overview_tests)
    set_tests_properties(qt-overview PROPERTIES TIMEOUT 20)

    add_executable(ctxfab_preferences_tests tests/PreferencesTests.cpp)
    target_link_libraries(ctxfab_preferences_tests PRIVATE
        ctxfab_qt_options ctxfab_qt_ui Qt6::Test)
    add_test(NAME qt-preferences COMMAND ctxfab_preferences_tests)
    set_tests_properties(qt-preferences PROPERTIES TIMEOUT 30)

    add_executable(ctxfab_github_tests tests/GitHubTests.cpp)
    target_link_libraries(ctxfab_github_tests PRIVATE
        ctxfab_qt_options ctxfab_qt_ui Qt6::Test)
    target_compile_definitions(ctxfab_github_tests PRIVATE
        CTXFAB_GITHUB_FIXTURE="${CMAKE_CURRENT_SOURCE_DIR}/../../quality/github/github_fixture.py")
    add_test(NAME qt-github COMMAND ctxfab_github_tests)
    set_tests_properties(qt-github PROPERTIES TIMEOUT 100)

    add_executable(ctxfab_trace_tests tests/TraceInspectorTests.cpp)
    target_link_libraries(ctxfab_trace_tests PRIVATE
        ctxfab_qt_options ctxfab_qt_ui Qt6::Test)
    add_test(NAME qt-traces COMMAND ctxfab_trace_tests)
    set_tests_properties(qt-traces PROPERTIES TIMEOUT 20)

    add_executable(ctxfab_answers_tests tests/AnswersTests.cpp)
    target_link_libraries(ctxfab_answers_tests PRIVATE
        ctxfab_qt_options ctxfab_qt_ui Qt6::Test)
    target_compile_definitions(ctxfab_answers_tests PRIVATE
        CTXFAB_ANSWER_FIXTURE="${CMAKE_CURRENT_SOURCE_DIR}/../../quality/answers/mock_worker.py")
    add_test(NAME qt-answers COMMAND ctxfab_answers_tests)
    set_tests_properties(qt-answers PROPERTIES TIMEOUT 40)

    add_executable(ctxfab_models_tests tests/ModelLibraryTests.cpp)
    target_link_libraries(ctxfab_models_tests PRIVATE
        ctxfab_qt_options ctxfab_qt_ui Qt6::Test)
    target_compile_definitions(ctxfab_models_tests PRIVATE
        CTXFAB_HUB_FIXTURE="${CMAKE_CURRENT_SOURCE_DIR}/../../quality/models/hugging_face_fixture.py")
    add_test(NAME qt-models COMMAND ctxfab_models_tests)
    set_tests_properties(qt-models PROPERTIES TIMEOUT 65)

    add_executable(ctxfab_pipeline_tests tests/PipelineTests.cpp)
    target_link_libraries(ctxfab_pipeline_tests PRIVATE ctxfab_qt_options ctxfab_qt_ui Qt6::Test)
    add_test(NAME qt-pipeline COMMAND ctxfab_pipeline_tests)
    set_tests_properties(qt-pipeline PROPERTIES TIMEOUT 50)

    add_executable(ctxfab_inference_tests tests/InferenceControlsTests.cpp)
    target_link_libraries(ctxfab_inference_tests PRIVATE
        ctxfab_qt_options ctxfab_qt_ui Qt6::Test)
    add_test(NAME qt-inference-controls COMMAND ctxfab_inference_tests)
    set_tests_properties(qt-inference-controls PROPERTIES TIMEOUT 45)
endif()

if(CTXFAB_COVERAGE)
    include(cmake/Coverage.cmake)
endif()

if(CTXFAB_QUALITY_TOOLS)
    find_program(CTXFAB_FORMAT NAMES clang-format-19 clang-format REQUIRED)
    find_program(CTXFAB_LIZARD NAMES lizard REQUIRED)
    add_custom_target(ctxfab-format-check
        COMMAND "${CMAKE_COMMAND}" "-DSOURCE_DIR=${CMAKE_CURRENT_SOURCE_DIR}"
            "-DCLANG_FORMAT=${CTXFAB_FORMAT}" "-DQML_FORMAT=$<TARGET_FILE:Qt6::qmlformat>"
            -P "${CMAKE_CURRENT_SOURCE_DIR}/cmake/Format.cmake"
        VERBATIM)
    add_custom_target(ctxfab-complexity
        COMMAND "${CTXFAB_LIZARD}" -l cpp -C 10 "${CMAKE_CURRENT_SOURCE_DIR}/src"
        VERBATIM)
    add_custom_target(ctxfab-lint ALL
        DEPENDS ctxfab-format-check ctxfab-complexity ctxfab_qt_qmllint)
    if(TARGET ctxfab-coverage)
        add_dependencies(ctxfab-coverage ctxfab-lint)
    endif()
endif()
