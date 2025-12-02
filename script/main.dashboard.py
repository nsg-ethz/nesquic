#!/usr/bin/env -S uv tool run --from git+https://github.com/lbrndnr/grafanalib@main generate-dashboard -o docker/grafana/dashboard/main.json

from grafanalib.core import (
    BarChart,
    Dashboard,
    GridPos,
    RowPanel,
    Target,
)

PANEL_HEIGHT = 8
DASHBOARD_WIDTH = 24
DASHBOARD_MID = DASHBOARD_WIDTH / 2


def io_panels(library, mode, ym):
    num = BarChart(
        title=f"{mode.capitalize()} I/O Syscalls",
        dataSource="prometheus",
        orientation="vertical",
        targets=[
            Target(
                expr=f'io_syscalls_invocations_sum{{library="{library}", mode="{mode}"}}',
                format="table",
                instant=True,
            )
        ],
        showLegend=False,
        gridPos=GridPos(h=PANEL_HEIGHT, w=DASHBOARD_MID, x=0, y=ym * PANEL_HEIGHT),
        xField="syscall",
        axisLabel="Invocations",
    )

    vol = BarChart(
        title=f"{mode.capitalize()} I/O Data Volume",
        dataSource="prometheus",
        orientation="vertical",
        targets=[
            Target(
                expr=f'io_syscalls_data_volume_sum{{library="{library}", mode="{mode}"}}',
                format="table",
                instant=True,
            )
        ],
        showLegend=False,
        gridPos=GridPos(
            h=PANEL_HEIGHT, w=DASHBOARD_MID, x=DASHBOARD_MID, y=ym * PANEL_HEIGHT
        ),
        xField="syscall",
        axisLabel="Bytes",
    )

    return [vol, num]


def throughput_panel(library, ym):
    return BarChart(
        title="Throughput",
        dataSource="prometheus",
        orientation="vertical",
        targets=[
            Target(
                expr=f'throughput_sum{{library="{library}"}}',
                format="table",
                instant=True,
            )
        ],
        showLegend=False,
        gridPos=GridPos(h=PANEL_HEIGHT, w=DASHBOARD_WIDTH, x=0, y=ym * PANEL_HEIGHT),
        xField="job",
        axisLabel="Mbps",
    )


def library_panels(library):
    return [
        RowPanel(
            title=library.capitalize(),
        ),
        *io_panels(library, "server", 0),
        *io_panels(library, "client", 1),
        throughput_panel(library, 2),
    ]


dashboard = Dashboard(
    title="Nesquic",
    description="An overview dashboard for the Nesquic projecgt",
    timezone="browser",
    panels=library_panels("quinn"),
).auto_panel_ids()
