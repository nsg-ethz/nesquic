#!/usr/bin/env -S uv tool run --from git+https://github.com/lbrndnr/grafanalib@main generate-dashboard -o docker/grafana/dashboard/main.json

from grafanalib.core import (
    BarChart,
    Dashboard,
    GridPos,
    RowPanel,
    Target,
)


def io_panels(library):
    num = BarChart(
        title="I/O Syscalls",
        dataSource="prometheus",
        orientation="vertical",
        targets=[
            Target(
                expr=f'io_syscalls_invocations_sum{{library="{library}"}}',
                format="table",
                instant=True,
            )
        ],
        showLegend=False,
        gridPos=GridPos(h=8, w=12, x=0, y=0),
        xField="syscall",
        axisLabel="Invocations",
    )

    vol = BarChart(
        title="I/O Data Volume",
        dataSource="prometheus",
        orientation="vertical",
        targets=[
            Target(
                expr=f'io_syscalls_data_volume_sum{{library="{library}"}}',
                format="table",
                instant=True,
            )
        ],
        showLegend=False,
        gridPos=GridPos(h=8, w=12, x=12, y=0),
        xField="syscall",
        axisLabel="Bytes",
    )

    return [vol, num]


def library_panels(library):
    return [
        RowPanel(
            title=library.capitalize(),
        ),
        *io_panels(library),
    ]


dashboard = Dashboard(
    title="Nesquic",
    description="An overview dashboard for the Nesquic projecgt",
    timezone="browser",
    panels=library_panels("quinn"),
).auto_panel_ids()
