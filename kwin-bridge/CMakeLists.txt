cmake_minimum_required(VERSION 3.22)
project(desktop_idle_status_kwin_bridge VERSION 0.1.0 LANGUAGES CXX)

# Only distribution packages opt in. Development builds stay disabled.
option(DESKTOP_IDLE_STATUS_BRIDGE_ENABLED_BY_DEFAULT "Enable bridge at KWin startup" OFF)
file(READ "${CMAKE_CURRENT_SOURCE_DIR}/metadata.json" bridge_metadata)
if(DESKTOP_IDLE_STATUS_BRIDGE_ENABLED_BY_DEFAULT)
    string(REPLACE "\"EnabledByDefault\": false" "\"EnabledByDefault\": true" bridge_metadata "${bridge_metadata}")
endif()
file(WRITE "${CMAKE_CURRENT_BINARY_DIR}/metadata.json" "${bridge_metadata}")

set(CMAKE_CXX_STANDARD 20)
set(CMAKE_CXX_STANDARD_REQUIRED ON)
set(CMAKE_AUTOMOC ON)
set(BUILD_SHARED_LIBS ON)
find_package(ECM REQUIRED NO_MODULE)
list(APPEND CMAKE_MODULE_PATH ${ECM_MODULE_PATH})
set(QT_MAJOR_VERSION 6)
include(KDEInstallDirs6)
find_package(KWin 6.6.6 REQUIRED)
find_package(Qt6 REQUIRED COMPONENTS DBus Widgets)
find_package(KF6CoreAddons REQUIRED)

execute_process(COMMAND dpkg-query -W [=[-f=${Version}]=] libkwin6
    OUTPUT_VARIABLE KWIN_PACKAGE_VERSION OUTPUT_STRIP_TRAILING_WHITESPACE
    COMMAND_ERROR_IS_FATAL ANY)
configure_file(src/build-info.h.in build-info.h @ONLY)
set(CMAKE_LIBRARY_OUTPUT_DIRECTORY "${CMAKE_BINARY_DIR}/plugins")
kcoreaddons_add_plugin(desktopidlestatusbridge
    SOURCES src/main.cpp src/bridge.cpp src/bridge.h
    INSTALL_NAMESPACE "kwin/plugins")
# Keep the plugin in an isolated discovery root. Do not run cmake --install.
set_target_properties(desktopidlestatusbridge PROPERTIES
    PREFIX ""
    LIBRARY_OUTPUT_DIRECTORY "${CMAKE_BINARY_DIR}/plugins/kwin/plugins")
target_include_directories(desktopidlestatusbridge PRIVATE ${CMAKE_CURRENT_BINARY_DIR} ${CMAKE_CURRENT_SOURCE_DIR})
target_compile_options(desktopidlestatusbridge PRIVATE -Wall -Wextra)
target_link_libraries(desktopidlestatusbridge PRIVATE KWin::kwin Qt6::DBus KF6::CoreAddons)
