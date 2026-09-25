cmake_minimum_required(VERSION 3.16)
project(nesquic_lsquic C)

set(CMAKE_C_STANDARD 11)
set(CMAKE_C_STANDARD_REQUIRED ON)

# lsquic is linked statically; BoringSSL must be linked dynamically so the
# LD_PRELOAD monitor can interpose its crypto calls (see docs/CLI.md).
set(LSQUIC_PREFIX "/usr/local" CACHE PATH "lsquic install prefix")
set(BORINGSSL_PREFIX "/opt/boringssl" CACHE PATH "BoringSSL install prefix")

add_executable(nesquic-lsquic
    src/main.c
    src/common.c
    src/client.c
    src/server.c
)

find_package(ZLIB REQUIRED)
find_package(Threads REQUIRED)

target_compile_options(nesquic-lsquic PRIVATE -Wall -Wextra)
target_include_directories(nesquic-lsquic PRIVATE
    ${CMAKE_CURRENT_SOURCE_DIR}/../c-common
    ${LSQUIC_PREFIX}/include/lsquic
    ${BORINGSSL_PREFIX}/include
)
target_link_libraries(nesquic-lsquic PRIVATE
    ${LSQUIC_PREFIX}/lib/liblsquic.a
    ${BORINGSSL_PREFIX}/lib/libssl.so
    ${BORINGSSL_PREFIX}/lib/libcrypto.so
    ZLIB::ZLIB
    Threads::Threads
    m
)
set_target_properties(nesquic-lsquic PROPERTIES
    BUILD_RPATH ${BORINGSSL_PREFIX}/lib
)

install(TARGETS nesquic-lsquic RUNTIME DESTINATION bin)
