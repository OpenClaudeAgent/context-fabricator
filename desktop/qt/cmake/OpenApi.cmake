function(ctxfab_generate_openapi contract)
    find_package(Java REQUIRED COMPONENTS Runtime)

    set(generator_version 7.27.0)
    set(generator_sha256 db9c4b738727698748746144394cb7ee96512fe50166555baaeeca9311ee1dba)
    set(generator_name "openapi-generator-cli-${generator_version}.jar")
    set(cache_dir "${CMAKE_CURRENT_SOURCE_DIR}/build/tools")
    set(CTXFAB_OPENAPI_GENERATOR_JAR "${cache_dir}/${generator_name}"
        CACHE FILEPATH "Path to the pinned OpenAPI Generator JAR")

    if(NOT EXISTS "${CTXFAB_OPENAPI_GENERATOR_JAR}")
        if(NOT CTXFAB_OPENAPI_GENERATOR_JAR STREQUAL "${cache_dir}/${generator_name}")
            message(FATAL_ERROR "The supplied OpenAPI Generator JAR does not exist")
        endif()
        file(MAKE_DIRECTORY "${cache_dir}")
        file(LOCK "${cache_dir}/download.lock" GUARD FUNCTION TIMEOUT 120)
        if(NOT EXISTS "${CTXFAB_OPENAPI_GENERATOR_JAR}")
            set(generator_path "org/openapitools/openapi-generator-cli/${generator_version}/${generator_name}")
            foreach(repository IN ITEMS
                "https://repo.maven.apache.org/maven2"
                "https://maven-central.storage-download.googleapis.com/maven2")
                file(DOWNLOAD "${repository}/${generator_path}" "${CTXFAB_OPENAPI_GENERATOR_JAR}.tmp"
                    TLS_VERIFY ON TIMEOUT 120 INACTIVITY_TIMEOUT 20 STATUS download_status)
                list(GET download_status 0 download_code)
                if(download_code EQUAL 0)
                    break()
                endif()
                message(STATUS "OpenAPI Generator download failed from ${repository}: ${download_status}")
            endforeach()
            if(NOT download_code EQUAL 0)
                file(REMOVE "${CTXFAB_OPENAPI_GENERATOR_JAR}.tmp")
                message(FATAL_ERROR "OpenAPI Generator download failed: ${download_status}")
            endif()
            file(SHA256 "${CTXFAB_OPENAPI_GENERATOR_JAR}.tmp" download_hash)
            if(NOT download_hash STREQUAL generator_sha256)
                file(REMOVE "${CTXFAB_OPENAPI_GENERATOR_JAR}.tmp")
                message(FATAL_ERROR "Downloaded OpenAPI Generator JAR does not match the pinned SHA-256")
            endif()
            file(RENAME "${CTXFAB_OPENAPI_GENERATOR_JAR}.tmp" "${CTXFAB_OPENAPI_GENERATOR_JAR}")
        endif()
    endif()
    file(SHA256 "${CTXFAB_OPENAPI_GENERATOR_JAR}" actual_sha256)
    if(NOT actual_sha256 STREQUAL generator_sha256)
        message(FATAL_ERROR "OpenAPI Generator JAR does not match the pinned SHA-256")
    endif()

    set_property(DIRECTORY APPEND PROPERTY CMAKE_CONFIGURE_DEPENDS
        "${contract}" "${CTXFAB_OPENAPI_GENERATOR_JAR}")
    file(SHA256 "${contract}" contract_hash)
    file(SHA256 "${CMAKE_CURRENT_FUNCTION_LIST_FILE}" recipe_hash)
    string(SHA256 generation_id "${contract_hash}:${recipe_hash}")
    set(generated "${CMAKE_CURRENT_BINARY_DIR}/openapi")
    set(stamp "${generated}/generation-id")
    if(EXISTS "${stamp}")
        file(READ "${stamp}" previous_generation)
    endif()
    if(NOT generation_id STREQUAL previous_generation)
        file(REMOVE_RECURSE "${generated}")
        file(MAKE_DIRECTORY "${generated}")
        # Preserve explicit null for required nullable fields in the contract.
        # Keep the rest of the pinned model template intact.
        set(templates "${generated}/templates")
        file(ARCHIVE_EXTRACT INPUT "${CTXFAB_OPENAPI_GENERATOR_JAR}"
            DESTINATION "${templates}" PATTERNS "cpp-qt-client/model-body.mustache")
        set(model_template "${templates}/cpp-qt-client/model-body.mustache")
        file(READ "${model_template}" model_body)
        set(parse_field [=[m_{{name}}_isValid = ::{{cppNamespace}}::fromJsonValue(m_{{name}}, json[QString("{{baseName}}")]);]=])
        set(write_field [=[obj.insert(QString("{{baseName}}"), ::{{cppNamespace}}::toJsonValue(m_{{name}}));
    }{{/isContainer}}]=])
        string(FIND "${model_body}" "${parse_field}" parse_offset)
        string(FIND "${model_body}" "${write_field}" write_offset)
        if(parse_offset LESS 0 OR write_offset LESS 0)
            message(FATAL_ERROR "The generated model template changed. Review nullable field handling.")
        endif()
        string(REPLACE "${parse_field}" [=[{{#required}}{{#isNullable}}m_{{name}} = {};
    {{/isNullable}}{{/required}}m_{{name}}_isValid = {{#required}}{{#isNullable}}json.value(QString("{{baseName}}")).isNull() || {{/isNullable}}{{/required}}::{{cppNamespace}}::fromJsonValue(m_{{name}}, json[QString("{{baseName}}")]);]=]
            model_body "${model_body}")
        string(REPLACE "${write_field}" [=[obj.insert(QString("{{baseName}}"), ::{{cppNamespace}}::toJsonValue(m_{{name}}));
    }{{#required}}{{#isNullable}} else if (m_{{name}}_isValid) {
        obj.insert(QString("{{baseName}}"), QJsonValue::Null);
    }{{/isNullable}}{{/required}}{{/isContainer}}]=]
            model_body "${model_body}")
        file(WRITE "${model_template}" "${model_body}")
        execute_process(
            COMMAND "${Java_JAVA_EXECUTABLE}" -jar "${CTXFAB_OPENAPI_GENERATOR_JAR}" generate
                -i "${contract}" -g cpp-qt-client -o "${generated}"
                -t "${templates}/cpp-qt-client"
                -p cppNamespace=ctxfab::openapi,modelNamePrefix=,enumUnknownDefaultCase=true,hideGenerationTimestamp=true
                --global-property apiDocs=false,modelDocs=false,apiTests=false,modelTests=false
            RESULT_VARIABLE result
            OUTPUT_FILE "${generated}/generate.log"
            ERROR_FILE "${generated}/generate-errors.log")
        if(NOT result STREQUAL "0")
            message(FATAL_ERROR "OpenAPI generation failed: see ${generated}/generate*.log")
        endif()

        # The loopback API returns JSON. Keep its response bytes in memory and
        # disable the generic template's implicit attachment writes.
        set(worker "${generated}/client/HttpRequest.cpp")
        file(READ "${worker}" transport)
        string(FIND "${transport}" "void HttpRequestWorker::process_response(QNetworkReply *reply) {" handler_start)
        string(FIND "${transport}" "QByteArray HttpRequestWorker::decompress(" handler_end)
        if(handler_start LESS 0 OR handler_end LESS_EQUAL handler_start)
            message(FATAL_ERROR "The generated HTTP transport changed. Review the JSON response handler.")
        endif()
        string(SUBSTRING "${transport}" 0 "${handler_start}" before_handler)
        string(SUBSTRING "${transport}" "${handler_end}" -1 after_handler)
        # The guarded model removal uses an OpenAPI DELETE body. Qt's
        # deleteResource overload has no way to pass that body through.
        set(delete_without_body "reply = manager->deleteResource(request);")
        string(FIND "${before_handler}" "${delete_without_body}" delete_offset)
        if(delete_offset LESS 0)
            message(FATAL_ERROR "The generated HTTP transport changed. Review DELETE body handling.")
        endif()
        string(REPLACE "${delete_without_body}" [=[reply = request_content.isEmpty()
            ? manager->deleteResource(request)
            : manager->sendCustomRequest(request, "DELETE", request_content);]=]
            before_handler "${before_handler}")
        file(WRITE "${worker}" "${before_handler}void HttpRequestWorker::process_response(QNetworkReply *reply) {
    response = reply->readAll();
}

${after_handler}")

        # The contract allows unsigned 32-bit GitHub revisions. The generator
        # defaults integer schemas without a format to signed 32-bit values.
        foreach(model GitHubAuthorization GitHubState GitHubSelection GitHubRateLimit
            GitHubStorageBudget SetGitHubStorage_request)
            foreach(extension h cpp)
                set(file "${generated}/client/${model}.${extension}")
                file(READ "${file}" contents)
                string(REGEX REPLACE
                    "qint32( (${model}::)?getRevision| &revision| m_revision;)"
                    "qint64\\1" contents "${contents}")
                file(WRITE "${file}" "${contents}")
            endforeach()
        endforeach()
        foreach(extension h cpp)
            set(file "${generated}/client/DefaultApi.${extension}")
            file(READ "${file}" contents)
            string(REGEX REPLACE
                "((checkGitHubRepositoryAccess|disconnectGitHub|listGitHubRepositoryBranches|removeGitHubRepository)\\([^\n]*const )qint32( &revision)"
                "\\1qint64\\3" contents "${contents}")
            file(WRITE "${file}" "${contents}")
            # The Rust collector represents GitHub run attempts as unsigned 32-bit values.
            set(file "${generated}/client/GitHubActionRun.${extension}")
            file(READ "${file}" contents)
            string(REGEX REPLACE "qint32( (GitHubActionRun::)?getAttempt| &attempt| m_attempt;)"
                "qint64\\1" contents "${contents}")
            file(WRITE "${file}" "${contents}")
        endforeach()

        # The generated signals copy DTOs by value, including on HTTP errors.
        # Initialize scalar storage even when a field is optional or invalid.
        file(GLOB model_headers "${generated}/client/*.h")
        foreach(header IN LISTS model_headers)
            file(READ "${header}" original)
            string(REGEX REPLACE "(\n    (bool|double|float|qint32|qint64) m_[A-Za-z0-9_]+);"
                "\\1{};" initialized "${original}")
            if(NOT original STREQUAL initialized)
                file(WRITE "${header}" "${initialized}")
            endif()
        endforeach()
        file(WRITE "${stamp}" "${generation_id}")
    endif()

    file(GLOB sources "${generated}/client/*.cpp" "${generated}/client/*.h")
    add_library(ctxfab_openapi STATIC ${sources})
    if(CTXFAB_OPENAPI_UNITY)
        # The generated models repeatedly include the same Qt and OpenAPI headers.
        set_target_properties(ctxfab_openapi PROPERTIES UNITY_BUILD ON UNITY_BUILD_BATCH_SIZE 8)
        set_source_files_properties(
            "${generated}/client/DefaultApi.cpp" "${generated}/client/HttpRequest.cpp"
            PROPERTIES SKIP_UNITY_BUILD_INCLUSION ON)
    endif()
    target_include_directories(ctxfab_openapi SYSTEM PUBLIC "${generated}/client")
    target_link_libraries(ctxfab_openapi PUBLIC Qt6::Core Qt6::Gui Qt6::Network)
endfunction()
