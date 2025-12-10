# pyright: reportCallIssue=none
import os

import yaml
from grafanalib.core import BarChart, Dashboard, GridPos, RowPanel, Target, Text

PANEL_HEIGHT = 8
DASHBOARD_WIDTH = 24
DASHBOARD_MID = DASHBOARD_WIDTH / 2
Y = 0


def y_offset():
    global Y

    res = Y
    Y += PANEL_HEIGHT
    return res


def format_labels(labels):
    return ", ".join(f"{k}='{v}'" for k, v in labels.items())


def io_panels(**labels):
    mode = labels["mode"]

    num = BarChart(
        title=f"{mode.capitalize()} I/O Syscalls",
        dataSource="prometheus",
        orientation="vertical",
        targets=[
            Target(
                expr=f"io_syscalls_invocations_sum{{{format_labels(labels)}}}",
                format="table",
                instant=True,
            )
        ],
        showLegend=False,
        gridPos=GridPos(h=PANEL_HEIGHT, w=DASHBOARD_MID, x=0, y=y_offset()),
        xField="syscall",
        axisLabel="Invocations",
    )

    vol = BarChart(
        title=f"{mode.capitalize()} I/O Data Volume",
        dataSource="prometheus",
        orientation="vertical",
        targets=[
            Target(
                expr=f"io_syscalls_data_volume_sum{{{format_labels(labels)}}}",
                format="table",
                instant=True,
            )
        ],
        showLegend=False,
        gridPos=GridPos(h=PANEL_HEIGHT, w=DASHBOARD_MID, x=DASHBOARD_MID, y=y_offset()),
        xField="syscall",
        axisLabel="Data Volume [kB]",
    )

    return [vol, num]


def throughput_panel(**labels):
    return BarChart(
        title="Throughput With Varying Connection Delay",
        dataSource="prometheus",
        orientation="vertical",
        targets=[
            Target(
                expr=f"throughput_sum{{{format_labels(labels)}}}",
                format="table",
                instant=True,
            )
        ],
        showLegend=False,
        gridPos=GridPos(h=PANEL_HEIGHT, w=DASHBOARD_WIDTH, x=0, y=y_offset()),
        xField="exported_job",
        axisLabel="Throughput [Mbps]",
    )


def overview_panels(**labels):
    return [
        RowPanel(
            title="Overview",
            gridPos=GridPos(h=1, w=DASHBOARD_WIDTH, x=0, y=y_offset()),
        ),
        throughput_panel(**labels),
    ]


def experiments_panels(experiment, **labels):
    return [
        RowPanel(
            title=experiment["title"],
            gridPos=GridPos(h=1, w=DASHBOARD_WIDTH, x=0, y=y_offset()),
        ),
        Text(
            content=experiment["description"],
            gridPos=GridPos(h=1.2, w=DASHBOARD_WIDTH, x=0, y=y_offset()),
        ),
        *io_panels(mode="server", **labels),
        *io_panels(mode="client", **labels),
    ]


def display_name(library):
    if library == "msquic":
        return "MsQuic"

    return library.capitalize()


library = os.environ.get("LIBRARY")
if library is None:
    raise ValueError("LIBRARY environment variable is not set")

labels = {"library": library, "log_level": "error"}

exps = os.environ.get("EXPERIMENTS")
if exps is None:
    raise ValueError("EXPERIMENTS environment variable is not set")

with open(exps, "r") as file:
    exps = yaml.safe_load(file)

exp_panels = [
    p
    for (j, e) in exps.items()
    for p in experiments_panels(e, exported_job=j, **labels)
]

dashboard = Dashboard(
    title=display_name(library),
    tags="nesquic",
    timezone="browser",
    panels=[*overview_panels(**labels), *exp_panels],
).auto_panel_ids()
