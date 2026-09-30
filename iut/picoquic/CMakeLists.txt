cmake_minimum_required(VERSION 3.16)
project(nesquic_picoquic C)

set(CMAKE_C_STANDARD 11)
set(CMAKE_C_STANDARD_REQUIRED ON)

# picoquic and picotls are linked statically; OpenSSL, which picotls uses
# for its crypto, is linked dynamically (see docs/CLI.md).
find_package(picoquic CONFIG REQUIRED)

add_executable(nesquic-picoquic src/main.c)

target_compile_options(nesquic-picoquic PRIVATE -Wall -Wextra)
target_include_directories(nesquic-picoquic PRIVATE ${CMAKE_CURRENT_SOURCE_DIR}/../c-common)
target_link_libraries(nesquic-picoquic PRIVATE picoquic::picoquic-core)

install(TARGETS nesquic-picoquic RUNTIME DESTINATION bin)
