#!/usr/bin/env python

import glob
import os
import pandas as pd
import re


def _get_file_paths(name, filename_pattern="*.json"):
    dir_path = os.path.dirname(os.path.realpath(__file__))
    return glob.glob(os.path.join(dir_path, "..", "res", "runs", name, filename_pattern))


def _parse_bpf_path(path):
    match = re.search(r"(\w+)-bpf-(\w+).*", path)
    if match is None:
        raise ValueError(f"Failed to parse BPF path {path}")

    library = match.group(1)
    func = match.group(2)

    return library, func


def _load_bpf_data(paths):
    dfs = []
    for p in paths:
        library, func = _parse_bpf_path(p)

        with open(p, "r") as file:
            text = file.read()
            pattern = r"total: (\d+) nsecs, count: (\d+)"
            match = re.search(pattern, text)

            if match is None:
                print(f"Could not find total/count in {p}")
                continue

            total = int(match.group(1))/10**9
            count = int(match.group(2))

            df = pd.DataFrame({"library": [library], "file": [p.split("/")[-2]], "total": [total], "count": [count], "func": [func]})
            dfs.append(df)

    return pd.concat(dfs).reset_index(drop=True)


if __name__ == "__main__":
    paths = _get_file_paths("**", "*-bpf-*.log")
    df = _load_bpf_data(paths)
    print(df)

    io = ["ipc", "read", "write", "epoll"]
    crypto = ["rustls"]

    io_dg10x = df[df["func"].isin(io) & (df["file"] == "dg10x")]["total"].sum()
    crypto_dg10x = df[df["func"].isin(crypto) & (df["file"] == "dg10x")]["total"].sum()

    io_dg40x = df[df["func"].isin(io) & (df["file"] == "dg40x")]["total"].sum()
    crypto_dg40x = df[df["func"].isin(crypto) & (df["file"] == "dg40x")]["total"].sum()

    print("------------------------------")
    print(f"dg10x io: {io_dg10x}, crypto: {crypto_dg10x}")
    print(f"dg40x io: {io_dg40x}, crypto: {crypto_dg40x}")
