#!/usr/bin/env -S uv tool run --from git+https://github.com/lbrndnr/grafanalib@main generate-dashboard -o docker/grafana/dashboard/main.json

from grafanalib.core import (
    BarChart,
    Dashboard,
    GridPos,
    Target,
)


def io_targets(library):
    return [
        Target(
            expr=f'io_syscalls_data_volume_sum{{library="{library}"}}',
            format="table",
            instant=True,
        )
    ]


dashboard = Dashboard(
    title="Nesquic",
    description="An overview dashboard for the Nesquic projecgt",
    timezone="browser",
    panels=[
        BarChart(
            title="Quinn I/O",
            dataSource="prometheus",
            orientation="vertical",
            targets=io_targets("quinn"),
            showLegend=False,
            gridPos=GridPos(h=8, w=16, x=0, y=0),
            xField="syscall",
        ),
    ],
).auto_panel_ids()
