# pyright: reportCallIssue=none
import os

from grafanalib.core import BarChart, Dashboard, GridPos, RowPanel, Target

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
        axisLabel="Bytes",
    )

    return [vol, num]


def throughput_panel(**labels):
    return BarChart(
        title="Throughput",
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
        axisLabel="Mbps",
    )


def overview_panels(**labels):
    return [
        RowPanel(
            title="Overview",
            gridPos=GridPos(h=1, w=DASHBOARD_WIDTH, x=0, y=y_offset()),
        ),
        throughput_panel(**labels),
    ]


def experiments_panels(title, **labels):
    return [
        RowPanel(
            title=title,
            gridPos=GridPos(h=1, w=DASHBOARD_WIDTH, x=0, y=y_offset()),
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

dashboard = Dashboard(
    title=display_name(library),
    tags="nesquic",
    timezone="browser",
    panels=[
        *overview_panels(library=library),
        *experiments_panels("Unbounded", library=library, exported_job="unbounded"),
        *experiments_panels("5ms Delay", library=library, exported_job="5ms delay"),
        *experiments_panels("20ms Delay", library=library, exported_job="20ms delay"),
    ],
).auto_panel_ids()
