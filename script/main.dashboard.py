# pyright: reportCallIssue=none
import os

import yaml
from grafanalib.core import (
    BarChart,
    Dashboard,
    GridPos,
    RowPanel,
    Templating,
    Text,
)

BUCKET = "nesquic"
DATASOURCE = "influxdb"
PANEL_HEIGHT = 8
DASHBOARD_WIDTH = 24
DASHBOARD_MID = DASHBOARD_WIDTH / 2
Y = 0

# Grafana template variable filter — kept as a plain string so that
# the ${...} syntax is not interpreted by Python's f-string engine.
RUN_FILTER = '  |> filter(fn: (r) => r.nesquic_run =~ /^${nesquic_run:regex}$/)'

NESQUIC_RUN_VARIABLE = {
    "name": "nesquic_run",
    "label": "Run",
    "type": "query",
    "datasource": {"type": "influxdb", "uid": "influxdb"},
    "query": (
        'import "influxdata/influxdb/schema"\n'
        'schema.tagValues(bucket: "nesquic", tag: "nesquic_run")'
    ),
    "refresh": 2,
    "includeAll": True,
    "allValue": ".*",
    "multi": False,
    "sort": 1,
    "current": {},
    "options": [],
    "hide": 0,
}


def y_offset():
    global Y
    res = Y
    Y += PANEL_HEIGHT
    return res


class FluxTarget:
    """A Grafana target that emits a Flux query for the InfluxDB datasource."""

    def __init__(self, query, ref_id="A"):
        self.query = query
        self.ref_id = ref_id

    def to_json_data(self):
        return {
            "datasource": {"type": "influxdb", "uid": "influxdb"},
            "hide": False,
            "query": self.query,
            "refId": self.ref_id,
        }


def flux_throughput_query(library):
    return "\n".join([
        f'from(bucket: "{BUCKET}")',
        "  |> range(start: v.timeRangeStart, stop: v.timeRangeStop)",
        '  |> filter(fn: (r) => r._measurement == "nesquic" and r._field == "throughput")',
        f'  |> filter(fn: (r) => r.library == "{library}")',
        RUN_FILTER,
        '  |> rename(columns: {"_value": "throughput"})',
        "  |> group()",
    ])


def flux_io_query(library, mode, job, field):
    rename_to = "count" if field == "count" else "volume_kb_sum"
    return "\n".join([
        f'from(bucket: "{BUCKET}")',
        "  |> range(start: v.timeRangeStart, stop: v.timeRangeStop)",
        f'  |> filter(fn: (r) => r._measurement == "nesquic_io" and r._field == "{field}")',
        f'  |> filter(fn: (r) => r.library == "{library}" and r.mode == "{mode}" and r.job == "{job}")',
        RUN_FILTER,
        f'  |> rename(columns: {{"_value": "{rename_to}"}})',
        "  |> group()",
    ])


def io_panels(library, mode, job):
    num = BarChart(
        title=f"{mode.capitalize()} I/O Syscalls",
        dataSource=DATASOURCE,
        orientation="vertical",
        targets=[FluxTarget(flux_io_query(library, mode, job, "count"))],
        showLegend=False,
        gridPos=GridPos(h=PANEL_HEIGHT, w=DASHBOARD_MID, x=0, y=y_offset()),
        xField="syscall",
        axisLabel="Invocations",
    )

    vol = BarChart(
        title=f"{mode.capitalize()} I/O Data Volume",
        dataSource=DATASOURCE,
        orientation="vertical",
        targets=[FluxTarget(flux_io_query(library, mode, job, "volume_kb_sum"))],
        showLegend=False,
        gridPos=GridPos(h=PANEL_HEIGHT, w=DASHBOARD_MID, x=DASHBOARD_MID, y=y_offset()),
        xField="syscall",
        axisLabel="Data Volume [kB]",
    )

    return [num, vol]


def throughput_panel(library):
    return BarChart(
        title="Throughput With Varying Connection Delay",
        dataSource=DATASOURCE,
        orientation="vertical",
        targets=[FluxTarget(flux_throughput_query(library))],
        showLegend=False,
        gridPos=GridPos(h=PANEL_HEIGHT, w=DASHBOARD_WIDTH, x=0, y=y_offset()),
        xField="job",
        axisLabel="Throughput [Mbps]",
    )


def overview_panels(library):
    return [
        RowPanel(
            title="Overview",
            gridPos=GridPos(h=1, w=DASHBOARD_WIDTH, x=0, y=y_offset()),
        ),
        throughput_panel(library),
    ]


def experiments_panels(experiment, library):
    job = experiment["job"]
    return [
        RowPanel(
            title=experiment["title"],
            gridPos=GridPos(h=1, w=DASHBOARD_WIDTH, x=0, y=y_offset()),
        ),
        Text(
            content=experiment["description"],
            gridPos=GridPos(h=1.2, w=DASHBOARD_WIDTH, x=0, y=y_offset()),
        ),
        *io_panels(library, "server", job),
        *io_panels(library, "client", job),
    ]


def display_name(library):
    if library == "msquic":
        return "MsQuic"
    return library.capitalize()


library = os.environ.get("LIBRARY")
if library is None:
    raise ValueError("LIBRARY environment variable is not set")

exps_path = os.environ.get("EXPERIMENTS")
if exps_path is None:
    raise ValueError("EXPERIMENTS environment variable is not set")

with open(exps_path, "r") as f:
    exps = yaml.safe_load(f)

ov_panels = overview_panels(library)
exp_panels = [p for e in exps for p in experiments_panels(e, library)]

dashboard = Dashboard(
    title=display_name(library),
    tags="nesquic",
    timezone="browser",
    panels=[*ov_panels, *exp_panels],
    templating=Templating(list=[NESQUIC_RUN_VARIABLE]),
).auto_panel_ids()
