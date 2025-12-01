#!/usr/bin/env -S uv tool run --from grafanalib generate-dashboard -o docker/grafana/dashboard/main.json

from grafanalib.core import (
    BarChart,
    Dashboard,
    GridPos,
    Target,
)

dashboard = Dashboard(
    title="Nesquic",
    description="An overview dashboard for the Nesquic project",
    timezone="browser",
    panels=[
        BarChart(
            title="I/O Overview",
            dataSource="prometheus",
            targets=[
                Target(
                    expr="io_send_data_size_count",
                ),
                Target(
                    expr="io_sendto_data_size_count",
                ),
            ],
            gridPos=GridPos(h=8, w=16, x=0, y=0),
        ),
    ],
).auto_panel_ids()
