# pyright: reportCallIssue=none
import argparse
import json
import os
import sys

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
from grafanalib._gen import write_dashboard

BUCKET = "nesquic"
DATASOURCE = "influxdb"
PANEL_HEIGHT = 8
DASHBOARD_WIDTH = 24
DASHBOARD_MID = DASHBOARD_WIDTH / 2
Y = 0

MODE_COLORS = {"client": "green", "server": "blue"}
COLOR_BY_MODE = {
    "mappings": [{
        "type": "value",
        "options": {m: {"color": c, "index": i} for i, (m, c) in enumerate(MODE_COLORS.items())},
    }],
    "extraJson": {"options": {"colorByField": "mode"}},
}

# Grafana template variable filter — kept as a plain string so that
# the ${...} syntax is not interpreted by Python's f-string engine.
RUN_FILTER = '  |> filter(fn: (r) => r.nesquic_run =~ /^${nesquic_run:regex}$/)'

def nesquic_run_variable(library=None):
    # Newest run first; Grafana selects the first option by default.
    library_filter = [f'  |> filter(fn: (r) => r.library == "{library}")'] if library else []
    return {
        "name": "nesquic_run",
        "label": "Run",
        "type": "query",
        "datasource": {"type": "influxdb", "uid": "influxdb"},
        "query": "\n".join([
            f'from(bucket: "{BUCKET}")',
            "  |> range(start: 0)",
            *library_filter,
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


def mean_per(*columns):
    # Averages the repetitions (NQ_REPETITIONS in script/run.sh) of the invocation.
    return "\n".join([
        f'  |> group(columns: {json.dumps(columns)})',
        "  |> mean()",
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
        mean_per("job"),
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
        mean_per("syscall"),
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
        colorMode="fixed",
        fixedColor=MODE_COLORS[mode],
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
        colorMode="fixed",
        fixedColor=MODE_COLORS[mode],
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
        mean_per("mode"),
        f'  |> rename(columns: {{"_value": "{field}"}})',
        "  |> group()",
    ])


def flux_packets_dropped_query(library, job):
    return "\n".join([
        latest_invocation(library),
        f'from(bucket: "{BUCKET}")',
        "  |> range(start: 0)",
        '  |> filter(fn: (r) => r._measurement == "nesquic_quic")',
        '  |> filter(fn: (r) => (r._field == "packets_sent" and r.mode == "server") or (r._field == "packets_received" and r.mode == "client"))',
        f'  |> filter(fn: (r) => r.library == "{library}" and r.job == "{job}")',
        RUN_FILTER,
        INVOCATION_FILTER,
        mean_per("_field", "job"),
        "  |> group()",
        '  |> pivot(rowKey: ["job"], columnKey: ["_field"], valueColumn: "_value")',
        '  |> map(fn: (r) => ({mode: "client", packets_dropped: r.packets_sent - r.packets_received}))',
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
        **COLOR_BY_MODE,
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
        **COLOR_BY_MODE,
        axisLabel="Packets",
    )

    y = y_offset()
    received = BarChart(
        title="Packets Received",
        dataSource=DATASOURCE,
        orientation="vertical",
        targets=[FluxTarget(
            flux_quic_query(library, job, "packets_received")
            + '\n  |> filter(fn: (r) => r.mode == "client")'
        )],
        showLegend=False,
        gridPos=GridPos(h=PANEL_HEIGHT, w=DASHBOARD_MID, x=0, y=y),
        xField="mode",
        **COLOR_BY_MODE,
        axisLabel="Packets",
    )

    dropped = BarChart(
        title="Packets Dropped",
        dataSource=DATASOURCE,
        orientation="vertical",
        targets=[FluxTarget(flux_packets_dropped_query(library, job))],
        showLegend=False,
        gridPos=GridPos(h=PANEL_HEIGHT, w=DASHBOARD_MID, x=DASHBOARD_MID, y=y),
        xField="mode",
        **COLOR_BY_MODE,
        axisLabel="Packets",
    )

    return [acks, packets, received, dropped]


def throughput_panel(library):
    return BarChart(
        title="Throughput With Varying Connection Delay",
        dataSource=DATASOURCE,
        orientation="vertical",
        targets=[FluxTarget(flux_throughput_query(library))],
        showLegend=False,
        gridPos=GridPos(h=PANEL_HEIGHT, w=DASHBOARD_WIDTH, x=0, y=y_offset()),
        xField="job",
        colorMode="fixed",
        fixedColor="red",
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
        mean_per("job"),
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
        colorMode="fixed",
        fixedColor="red",
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
        colorMode="fixed",
        fixedColor="red",
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


def flux_comparison_query(measurement, field, job):
    # Per library: average the repetitions of each invocation, then keep the
    # newest invocation. Untagged (older) results count as invocation "".
    names = ", ".join(f'"{k}": "{v}"' for k, v in DISPLAY_NAMES.items())
    return "\n".join([
        'import "dict"',
        'import "strings"',
        f"names = [{names}]",
        f'from(bucket: "{BUCKET}")',
        "  |> range(start: 0)",
        f'  |> filter(fn: (r) => r._measurement == "{measurement}" and r._field == "{field}")',
        f'  |> filter(fn: (r) => r.job == "{job}")',
        RUN_FILTER,
        '  |> map(fn: (r) => ({r with nesquic_invocation: if exists r.nesquic_invocation then r.nesquic_invocation else ""}))',
        '  |> group(columns: ["library", "nesquic_invocation"])',
        "  |> mean()",
        '  |> group(columns: ["library"])',
        '  |> sort(columns: ["nesquic_invocation"])',
        "  |> last()",
        "  |> group()",
        '  |> sort(columns: ["library"])',
        "  |> map(fn: (r) => ({r with library: dict.get(dict: names, key: r.library, default: strings.title(v: r.library))}))",
        f'  |> rename(columns: {{"_value": "{field}"}})',
    ])


def comparison_chart(title, query, axis_label, x, y):
    return BarChart(
        title=title,
        dataSource=DATASOURCE,
        orientation="vertical",
        targets=[FluxTarget(query)],
        showLegend=False,
        gridPos=GridPos(h=PANEL_HEIGHT, w=DASHBOARD_MID, x=x, y=y),
        xField="library",
        colorMode="fixed",
        fixedColor="red",
        axisLabel=axis_label,
    )


def comparison_panels(experiment):
    job = experiment["job"]
    row = RowPanel(
        title=experiment["title"],
        gridPos=GridPos(h=1, w=DASHBOARD_WIDTH, x=0, y=y_offset()),
    )
    y = y_offset()
    return [
        row,
        comparison_chart(
            "Request Latency",
            flux_comparison_query("nesquic_latency", "request_latency_ms", job),
            "Request latency [ms]",
            0,
            y,
        ),
        comparison_chart(
            "Throughput",
            flux_comparison_query("nesquic", "throughput", job),
            "Throughput [Mbps]",
            DASHBOARD_MID,
            y,
        ),
    ]


def iut_dashboard(library, exps):
    return Dashboard(
        title=display_name(library),
        tags="nesquic",
        timezone="browser",
        timePicker=attr.evolve(DEFAULT_TIME_PICKER, hidden=True),
        panels=[
            *overview_panels(library),
            *(p for e in exps for p in experiments_panels(e, library)),
        ],
        templating=Templating(list=[nesquic_run_variable(library)]),
    ).auto_panel_ids()


def overview_dashboard(exps):
    return Dashboard(
        title="Overview",
        tags="nesquic",
        timezone="browser",
        timePicker=attr.evolve(DEFAULT_TIME_PICKER, hidden=True),
        panels=[p for e in exps for p in comparison_panels(e)],
        templating=Templating(list=[nesquic_run_variable()]),
    ).auto_panel_ids()


parser = argparse.ArgumentParser()
parser.add_argument("dashboard", help='library name, or "overview"')
parser.add_argument("-o", "--output", type=argparse.FileType("w"), default=sys.stdout)
parser.add_argument(
    "--experiments",
    default=os.path.join(os.path.dirname(__file__), "..", "res", "experiments.yaml"),
)
args = parser.parse_args()

with open(args.experiments, "r") as f:
    exps = yaml.safe_load(f)

if args.dashboard == "overview":
    dashboard = overview_dashboard(exps)
else:
    dashboard = iut_dashboard(args.dashboard, exps)

write_dashboard(dashboard, args.output)
