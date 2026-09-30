cmake_minimum_required(VERSION 3.16)
project(nesquic_xquic C)

set(CMAKE_C_STANDARD 11)
set(CMAKE_C_STANDARD_REQUIRED ON)

# xquic is linked statically; BoringSSL must be linked dynamically so the
# LD_PRELOAD monitor can interpose its crypto calls (see docs/CLI.md).
set(XQUIC_SOURCE_DIR "/xquic" CACHE PATH "xquic source tree")
set(XQUIC_LIBRARY "${XQUIC_SOURCE_DIR}/build/libxquic-static.a" CACHE FILEPATH "libxquic-static.a")
set(BORINGSSL_PREFIX "/opt/boringssl" CACHE PATH "BoringSSL install prefix")

add_executable(nesquic-xquic
    src/main.c
    src/common.c
    src/client.c
    src/server.c
)

find_package(Threads REQUIRED)

target_compile_options(nesquic-xquic PRIVATE -Wall -Wextra)
target_include_directories(nesquic-xquic PRIVATE
    ${CMAKE_CURRENT_SOURCE_DIR}/../c-common
    ${XQUIC_SOURCE_DIR}/include
    ${XQUIC_SOURCE_DIR}/build/include
    ${BORINGSSL_PREFIX}/include
)
target_link_libraries(nesquic-xquic PRIVATE
    ${XQUIC_LIBRARY}
    ${BORINGSSL_PREFIX}/lib/libssl.so
    ${BORINGSSL_PREFIX}/lib/libcrypto.so
    Threads::Threads
    m
    dl
)
set_target_properties(nesquic-xquic PROPERTIES
    BUILD_RPATH ${BORINGSSL_PREFIX}/lib
)

install(TARGETS nesquic-xquic RUNTIME DESTINATION bin)
