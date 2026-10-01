# pyright: reportCallIssue=none
import os

import attr
import yaml
from grafanalib.core import (
    DEFAULT_TIME_PICKER,
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

def nesquic_run_variable(library):
    # Newest run first; Grafana selects the first option by default.
    return {
        "name": "nesquic_run",
        "label": "Run",
        "type": "query",
        "datasource": {"type": "influxdb", "uid": "influxdb"},
        "query": "\n".join([
            f'from(bucket: "{BUCKET}")',
            "  |> range(start: 0)",
            f'  |> filter(fn: (r) => r.library == "{library}")',
            '  |> keep(columns: ["_time", "nesquic_run"])',
            '  |> group(columns: ["nesquic_run"])',
            '  |> max(column: "_time")',
            "  |> group()",
            '  |> sort(columns: ["_time"], desc: true)',
            "  |> map(fn: (r) => ({_value: r.nesquic_run}))",
        ]),
        "refresh": 1,
        "includeAll": False,
        "multi": False,
        "sort": 0,
        "current": {},
        "options": [],
        "hide": 0,
    }


def latest_invocation(library):
    # Results uploaded before script/run.sh tagged its invocations count as
    # invocation "".
    return "\n".join([
        'import "array"',
        'import "influxdata/influxdb/schema"',
        "latest = (union(tables: [",
        '    array.from(rows: [{_value: ""}]),',
        f'    schema.tagValues(bucket: "{BUCKET}", tag: "nesquic_invocation", start: 0,',
        f'      predicate: (r) => r.library == "{library}" and r.nesquic_run =~ /^${{nesquic_run:regex}}$/),',
        "  ]) |> sort() |> last() |> findRecord(fn: (key) => true, idx: 0))._value",
    ])


INVOCATION_FILTER = '  |> filter(fn: (r) => (if exists r.nesquic_invocation then r.nesquic_invocation else "") == latest)'


def last_per(column):
    return "\n".join([
        f'  |> group(columns: ["{column}"])',
        '  |> sort(columns: ["_time"])',
        "  |> last()",
    ])


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
        latest_invocation(library),
        f'from(bucket: "{BUCKET}")',
        "  |> range(start: 0)",
        '  |> filter(fn: (r) => r._measurement == "nesquic" and r._field == "throughput")',
        f'  |> filter(fn: (r) => r.library == "{library}")',
        RUN_FILTER,
        INVOCATION_FILTER,
        last_per("job"),
        '  |> rename(columns: {"_value": "throughput"})',
        "  |> group()",
    ])


def flux_io_query(library, mode, job, field):
    rename_to = "count" if field == "count" else "volume_kb_sum"
    return "\n".join([
        latest_invocation(library),
        f'from(bucket: "{BUCKET}")',
        "  |> range(start: 0)",
        f'  |> filter(fn: (r) => r._measurement == "nesquic_io" and r._field == "{field}")',
        f'  |> filter(fn: (r) => r.library == "{library}" and r.mode == "{mode}" and r.job == "{job}")',
        RUN_FILTER,
        INVOCATION_FILTER,
        last_per("syscall"),
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


def flux_quic_query(library, job, field):
    return "\n".join([
        latest_invocation(library),
        f'from(bucket: "{BUCKET}")',
        "  |> range(start: 0)",
        f'  |> filter(fn: (r) => r._measurement == "nesquic_quic" and r._field == "{field}")',
        f'  |> filter(fn: (r) => r.library == "{library}" and r.job == "{job}")',
        RUN_FILTER,
        INVOCATION_FILTER,
        last_per("mode"),
        f'  |> rename(columns: {{"_value": "{field}"}})',
        "  |> group()",
    ])


def quic_panels(library, job):
    """Packet and ACK counts, available for libraries whose crypto is hooked."""
    y = y_offset()
    acks = BarChart(
        title="ACK Frames Sent",
        dataSource=DATASOURCE,
        orientation="vertical",
        targets=[FluxTarget(flux_quic_query(library, job, "acks_sent"))],
        showLegend=False,
        gridPos=GridPos(h=PANEL_HEIGHT, w=DASHBOARD_MID, x=0, y=y),
        xField="mode",
        axisLabel="ACK frames",
    )

    packets = BarChart(
        title="Packets Sent",
        dataSource=DATASOURCE,
        orientation="vertical",
        targets=[FluxTarget(flux_quic_query(library, job, "packets_sent"))],
        showLegend=False,
        gridPos=GridPos(h=PANEL_HEIGHT, w=DASHBOARD_MID, x=DASHBOARD_MID, y=y),
        xField="mode",
        axisLabel="Packets",
    )

    return [acks, packets]


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


def flux_latency_query(library, field):
    return "\n".join([
        latest_invocation(library),
        f'from(bucket: "{BUCKET}")',
        "  |> range(start: 0)",
        f'  |> filter(fn: (r) => r._measurement == "nesquic_latency" and r._field == "{field}")',
        f'  |> filter(fn: (r) => r.library == "{library}")',
        RUN_FILTER,
        INVOCATION_FILTER,
        last_per("job"),
        f'  |> rename(columns: {{"_value": "{field}"}})',
        "  |> group()",
    ])


def latency_panels(library):
    """TTFB and request latency, available for libraries whose crypto is hooked."""
    y = y_offset()
    ttfb = BarChart(
        title="Time to First Byte",
        dataSource=DATASOURCE,
        orientation="vertical",
        targets=[FluxTarget(flux_latency_query(library, "ttfb_ms"))],
        showLegend=False,
        gridPos=GridPos(h=PANEL_HEIGHT, w=DASHBOARD_MID, x=0, y=y),
        xField="job",
        axisLabel="TTFB [ms]",
    )

    request = BarChart(
        title="Request Latency",
        dataSource=DATASOURCE,
        orientation="vertical",
        targets=[FluxTarget(flux_latency_query(library, "request_latency_ms"))],
        showLegend=False,
        gridPos=GridPos(h=PANEL_HEIGHT, w=DASHBOARD_MID, x=DASHBOARD_MID, y=y),
        xField="job",
        axisLabel="Request latency [ms]",
    )

    return [ttfb, request]


def overview_panels(library):
    return [
        RowPanel(
            title="Overview",
            gridPos=GridPos(h=1, w=DASHBOARD_WIDTH, x=0, y=y_offset()),
        ),
        throughput_panel(library),
        *latency_panels(library),
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
        *quic_panels(library, job),
    ]


DISPLAY_NAMES = {
    "msquic": "MsQuic",
    "ngtcp2": "ngtcp2",
    "lsquic": "LSQUIC",
    "xquic": "XQUIC",
    "picoquic": "picoquic",
    "mvfst": "mvfst",
}


def display_name(library):
    return DISPLAY_NAMES.get(library, library.capitalize())


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
    timePicker=attr.evolve(DEFAULT_TIME_PICKER, hidden=True),
    panels=[*ov_panels, *exp_panels],
    templating=Templating(list=[nesquic_run_variable(library)]),
).auto_panel_ids()
