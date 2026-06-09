cmake_minimum_required(VERSION 3.16)
project(nesquic_msquic CXX)

set(CMAKE_CXX_STANDARD 17)
set(CMAKE_CXX_STANDARD_REQUIRED ON)

add_executable(nesquic-msquic
    src/main.cpp
    src/common.cpp
    src/client.cpp
    src/server.cpp
)

target_compile_options(nesquic-msquic PRIVATE -Wall -Wextra)

# libmsquic and its headers are installed system-wide (see Dockerfile.msquic).
# Allow overriding the include/library locations for non-standard installs.
if(DEFINED MSQUIC_INCLUDE_DIR)
    target_include_directories(nesquic-msquic PRIVATE ${MSQUIC_INCLUDE_DIR})
endif()

find_library(MSQUIC_LIB msquic)
if(NOT MSQUIC_LIB)
    message(FATAL_ERROR "libmsquic not found")
endif()

find_package(Threads REQUIRED)

target_link_libraries(nesquic-msquic PRIVATE ${MSQUIC_LIB} Threads::Threads)

install(TARGETS nesquic-msquic RUNTIME DESTINATION bin)
