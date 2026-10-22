cmake_minimum_required(VERSION 3.24)
project(TransmogSDK VERSION 0.1.0 LANGUAGES CXX)
include(GNUInstallDirs)
include(CMakePackageConfigHelpers)
include(CTest)

foreach(required TRANSMOG_RUST_LIBRARY TRANSMOG_GENERATED_DIR TRANSMOG_CRUBIT_ROOT)
  if(NOT DEFINED ${required})
    message(FATAL_ERROR "Build the SDK using python scripts/build-sdk.py (${required} is missing)")
  endif()
endforeach()

add_library(transmog_sdk SHARED src/sdk.cpp src/c_api.cpp)
add_library(Transmog::SDK ALIAS transmog_sdk)
set_target_properties(transmog_sdk PROPERTIES EXPORT_NAME SDK CXX_VISIBILITY_PRESET hidden
  VISIBILITY_INLINES_HIDDEN YES MSVC_RUNTIME_LIBRARY MultiThreadedDLL)
target_compile_features(transmog_sdk PUBLIC cxx_std_20)
target_compile_definitions(transmog_sdk PRIVATE TRANSMOG_SDK_BUILD)
target_include_directories(transmog_sdk PUBLIC
  $<BUILD_INTERFACE:${CMAKE_CURRENT_SOURCE_DIR}/include> $<INSTALL_INTERFACE:${CMAKE_INSTALL_INCLUDEDIR}>)
target_include_directories(transmog_sdk SYSTEM PRIVATE "${TRANSMOG_GENERATED_DIR}" "${TRANSMOG_CRUBIT_ROOT}")
target_link_libraries(transmog_sdk PRIVATE "${TRANSMOG_RUST_LIBRARY}")
target_link_directories(transmog_sdk PRIVATE ${TRANSMOG_NATIVE_DIRS})
foreach(native_lib IN LISTS TRANSMOG_NATIVE_LIBS)
  if(native_lib MATCHES "^/")
    target_link_options(transmog_sdk PRIVATE "${native_lib}")
  else()
    target_link_libraries(transmog_sdk PRIVATE "${native_lib}")
  endif()
endforeach()
if(MSVC)
  target_compile_options(transmog_sdk PRIVATE /W4 /WX /EHsc)
else()
  target_compile_options(transmog_sdk PRIVATE -Wall -Wextra -Werror)
  # Rust static-library symbols are private implementation details.
  target_link_options(transmog_sdk PRIVATE -Wl,--exclude-libs,ALL)
endif()

install(TARGETS transmog_sdk EXPORT TransmogTargets
  RUNTIME DESTINATION ${CMAKE_INSTALL_BINDIR}
  LIBRARY DESTINATION ${CMAKE_INSTALL_LIBDIR}
  ARCHIVE DESTINATION ${CMAKE_INSTALL_LIBDIR})
install(DIRECTORY include/ DESTINATION ${CMAKE_INSTALL_INCLUDEDIR})
install(EXPORT TransmogTargets FILE TransmogTargets.cmake NAMESPACE Transmog::
  DESTINATION ${CMAKE_INSTALL_LIBDIR}/cmake/Transmog)
configure_package_config_file(cmake/TransmogConfig.cmake.in "${CMAKE_CURRENT_BINARY_DIR}/TransmogConfig.cmake"
  INSTALL_DESTINATION ${CMAKE_INSTALL_LIBDIR}/cmake/Transmog)
install(FILES "${CMAKE_CURRENT_BINARY_DIR}/TransmogConfig.cmake"
  DESTINATION ${CMAKE_INSTALL_LIBDIR}/cmake/Transmog)
install(FILES "${CMAKE_CURRENT_SOURCE_DIR}/../LICENSE" DESTINATION share/transmog)
install(FILES "${TRANSMOG_CRUBIT_ROOT}/LICENSE" DESTINATION share/transmog RENAME CRUBIT-LICENSE)

if(BUILD_TESTING)
  add_executable(sdk_contract tests/contract.cpp src/sdk.cpp src/c_api.cpp)
  target_compile_definitions(sdk_contract PRIVATE TRANSMOG_SDK_BUILD)
  target_compile_features(sdk_contract PRIVATE cxx_std_20)
  target_include_directories(sdk_contract PRIVATE include tests/fake)
  if(MSVC)
    target_compile_options(sdk_contract PRIVATE /W4 /WX /EHsc)
  else()
    target_compile_options(sdk_contract PRIVATE -Wall -Wextra -Werror)
  endif()
  add_test(NAME sdk_no_throw_contract COMMAND sdk_contract)
endif()
