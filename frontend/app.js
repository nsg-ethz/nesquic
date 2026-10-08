// Nesquic results: the panels of the former Grafana dashboard
// (script/main.dashboard.py), drawn with D3 as mean ± standard deviation over
// all matching measurements, plus a qvis view of each experiment's qlog.
import * as d3 from "d3";
import yaml from "js-yaml";

const BUCKET = "nesquic";
const ALL = "__all__";
const EPOCH = "1970-01-01T00:00:00Z";

const els = {
    library: document.getElementById("library"),
    run: document.getElementById("run"),
    range: document.getElementById("range"),
    refresh: document.getElementById("refresh"),
    status: document.getElementById("status"),
    dashboard: document.getElementById("dashboard"),
    tooltip: document.getElementById("tooltip"),
};

let experiments = [];
const charts = [];

// --- InfluxDB ---------------------------------------------------------------

function fluxString(s) {
    return '"' + String(s).replace(/[\\"$]/g, (c) => "\\" + c) + '"';
}

async function flux(query) {
    const res = await fetch("api/query", {
        method: "POST",
        headers: { "Content-Type": "application/json", Accept: "application/csv" },
        body: JSON.stringify({
            query,
            type: "flux",
            dialect: { header: true, annotations: [] },
        }),
    });
    const text = await res.text();
    if (!res.ok) {
        let msg = text;
        try { msg = JSON.parse(text).message; } catch (_) { /* plain text */ }
        throw new Error(`InfluxDB: ${msg}`);
    }
    // Tables with different schemas are separated by blank lines, each with
    // its own header row.
    return text
        .split(/\r?\n\s*\r?\n/)
        .filter((t) => t.trim())
        .flatMap((t) => d3.csvParse(t.trim()));
}

async function tagValues(tag, predicate) {
    const rows = await flux([
        'import "influxdata/influxdb/schema"',
        `schema.tagValues(bucket: ${fluxString(BUCKET)}, tag: ${fluxString(tag)}, ` +
            `start: ${EPOCH}${predicate ? `, predicate: ${predicate}` : ""})`,
    ].join("\n"));
    return rows.map((r) => r._value).sort(d3.ascending);
}

/** All points of `measurement` for the selected library, run and time range. */
async function points(measurement, { library, run, range }) {
    const start = range === "all" ? EPOCH : range;
    const lines = [
        `from(bucket: ${fluxString(BUCKET)})`,
        `  |> range(start: ${start})`,
        `  |> filter(fn: (r) => r._measurement == ${fluxString(measurement)})`,
        `  |> filter(fn: (r) => r.library == ${fluxString(library)})`,
    ];
    if (run !== ALL) {
        lines.push(`  |> filter(fn: (r) => r.nesquic_run == ${fluxString(run)})`);
    }
    lines.push(
        "  |> toFloat()",
        '  |> keep(columns: ["_time", "_field", "_value", "job", "mode", "syscall", "nesquic_run"])',
        "  |> group()",
    );
    const rows = await flux(lines.join("\n"));
    for (const r of rows) r._value = +r._value;
    return rows;
}

// --- Aggregation ------------------------------------------------------------

/** Mean, sample standard deviation and count of `_value`, per `key(row)`. */
function summarize(rows, key, order) {
    const groups = d3.rollup(
        rows,
        (v) => {
            const values = v.map((r) => r._value);
            return {
                mean: d3.mean(values),
                std: values.length > 1 ? d3.deviation(values) : 0,
                n: values.length,
            };
        },
        key,
    );
    const rank = new Map((order || []).map((k, i) => [k, i]));
    return Array.from(groups, ([category, s]) => ({ category, ...s })).sort(
        (a, b) =>
            (rank.get(a.category) ?? Infinity) - (rank.get(b.category) ?? Infinity) ||
            d3.ascending(a.category, b.category),
    );
}

// --- Bar chart --------------------------------------------------------------

const fmt = (v) => (Math.abs(v) >= 1000 ? d3.format(",.0f")(v) : d3.format(",.3~f")(v));

function showTooltip(event, d, unit) {
    const t = els.tooltip;
    t.innerHTML = "";
    const title = document.createElement("strong");
    title.textContent = d.category;
    t.append(title);
    for (const [k, v] of [
        [unit, fmt(d.mean)],
        ["std. dev.", fmt(d.std)],
        ["samples", d.n],
    ]) {
        const line = document.createElement("div");
        const key = document.createElement("span");
        key.className = "k";
        key.textContent = `${k}: `;
        line.append(key, String(v));
        t.append(line);
    }
    t.hidden = false;
    const pad = 12;
    const { width, height } = t.getBoundingClientRect();
    let x = event.clientX + pad;
    let y = event.clientY + pad;
    if (x + width > window.innerWidth) x = event.clientX - width - pad;
    if (y + height > window.innerHeight) y = event.clientY - height - pad;
    t.style.left = `${x}px`;
    t.style.top = `${y}px`;
}

function hideTooltip() {
    els.tooltip.hidden = true;
}

function drawBarChart(container, data, yLabel) {
    container.innerHTML = "";
    if (!data.length) {
        const empty = document.createElement("div");
        empty.className = "empty";
        empty.textContent = "No data";
        container.append(empty);
        return;
    }

    const width = container.clientWidth;
    const height = container.clientHeight;
    const margin = { top: 10, right: 8, bottom: 28, left: 64 };
    const innerW = Math.max(0, width - margin.left - margin.right);
    const innerH = Math.max(0, height - margin.top - margin.bottom);

    const x = d3
        .scaleBand()
        .domain(data.map((d) => d.category))
        .range([0, innerW])
        .paddingInner(0.25)
        .paddingOuter(0.1);
    const yMax = d3.max(data, (d) => d.mean + d.std) || 1;
    const y = d3.scaleLinear().domain([0, yMax]).nice().range([innerH, 0]);

    const svg = d3
        .select(container)
        .append("svg")
        .attr("width", width)
        .attr("height", height)
        .attr("role", "img")
        .attr("aria-label", `${yLabel} by category`);
    const g = svg.append("g").attr("transform", `translate(${margin.left},${margin.top})`);

    g.append("g")
        .attr("class", "y-axis")
        .call(d3.axisLeft(y).ticks(5).tickSize(-innerW).tickFormat(d3.format("~s")))
        .call((a) => a.select(".domain").remove());

    g.append("g")
        .attr("class", "x-axis")
        .attr("transform", `translate(0,${innerH})`)
        .call(d3.axisBottom(x).tickSizeOuter(0));

    svg.append("text")
        .attr("class", "axis-label")
        .attr("transform", `translate(12,${margin.top + innerH / 2}) rotate(-90)`)
        .attr("text-anchor", "middle")
        .text(yLabel);

    // Bars: 4px rounded top, square at the baseline.
    const r = Math.min(4, x.bandwidth() / 2);
    const barPath = (d) => {
        const x0 = x(d.category);
        const x1 = x0 + x.bandwidth();
        const y0 = y(0);
        const y1 = y(d.mean);
        const rr = Math.min(r, y0 - y1);
        return `M${x0},${y0}V${y1 + rr}Q${x0},${y1} ${x0 + rr},${y1}` +
            `H${x1 - rr}Q${x1},${y1} ${x1},${y1 + rr}V${y0}Z`;
    };

    const bar = g.append("g").selectAll("g").data(data).join("g");

    bar.append("path").attr("class", "bar").attr("d", barPath);

    // Error bars: mean ± one standard deviation, clipped at zero.
    const cap = Math.min(10, x.bandwidth() / 3);
    bar.filter((d) => d.std > 0)
        .append("path")
        .attr("class", "err")
        .attr("d", (d) => {
            const cx = x(d.category) + x.bandwidth() / 2;
            const lo = y(Math.max(0, d.mean - d.std));
            const hi = y(d.mean + d.std);
            return `M${cx},${lo}V${hi}M${cx - cap / 2},${hi}H${cx + cap / 2}` +
                `M${cx - cap / 2},${lo}H${cx + cap / 2}`;
        });

    // Hit targets span the whole column, not just the bar.
    bar.append("rect")
        .attr("x", (d) => x(d.category) - (x.step() - x.bandwidth()) / 2)
        .attr("width", x.step())
        .attr("y", 0)
        .attr("height", innerH)
        .attr("fill", "transparent")
        .on("pointerenter pointermove", function (event, d) {
            d3.select(this.parentNode).select(".bar").classed("hover", true);
            showTooltip(event, d, yLabel);
        })
        .on("pointerleave", function () {
            d3.select(this.parentNode).select(".bar").classed("hover", false);
            hideTooltip();
        });
}

// --- Layout -----------------------------------------------------------------

function element(tag, props = {}, ...children) {
    const e = Object.assign(document.createElement(tag), props);
    e.append(...children);
    return e;
}

function panel(parent, title, yLabel, data, wide = false) {
    const chart = element("div", { className: "chart" });
    parent.append(element("div", { className: wide ? "panel wide" : "panel" },
        element("h3", { textContent: title }), chart));
    charts.push({ chart, data, yLabel });
}

function redraw() {
    for (const c of charts) drawBarChart(c.chart, c.data, c.yLabel);
}

function capitalize(s) {
    return s.charAt(0).toUpperCase() + s.slice(1);
}

function qvisUrl(view, file) {
    return `qvis/#/${view}?file=${encodeURIComponent(new URL(file, location.href).href)}`;
}

function qlogSection(library, job, available) {
    const file = `qlog/${encodeURIComponent(library)}/${encodeURIComponent(job)}.qlog`;
    const head = element("div", { className: "head" }, element("h3", { textContent: "qlog" }));
    const box = element("div", { className: "qlog" }, head);

    if (!available.has(`${job}.qlog`)) {
        head.append(element("span", {
            className: "muted",
            textContent: `No trace at res/qlog/${library}/${job}.qlog (run with NQ_QLOG=1).`,
        }));
        return box;
    }

    let frame = null;
    const view = element("select", {},
        ...["sequence", "congestion", "packetization", "multiplexing", "stats"].map((v) =>
            element("option", { value: v, textContent: capitalize(v) })));
    const toggle = element("button", { type: "button", textContent: "Show in qvis" });
    const open = element("a", { href: qvisUrl("sequence", file), target: "_blank", textContent: "Open in new tab" });
    const raw = element("a", { href: file, download: "", textContent: "Download" });

    const load = () => {
        open.href = qvisUrl(view.value, file);
        if (frame) frame.src = open.href;
    };
    view.addEventListener("change", load);
    toggle.addEventListener("click", () => {
        if (frame) {
            frame.remove();
            frame = null;
            toggle.textContent = "Show in qvis";
            return;
        }
        frame = element("iframe", { title: `qvis: ${library} ${job}`, loading: "lazy" });
        box.append(frame);
        toggle.textContent = "Hide";
        load();
    });

    head.append(view, toggle, open, raw);
    return box;
}

async function qlogFiles(library) {
    try {
        const res = await fetch(`qlog/${encodeURIComponent(library)}/`, { cache: "no-store" });
        if (!res.ok) return new Set();
        const listing = await res.json();
        return new Set(listing.filter((f) => f.type === "file").map((f) => f.name));
    } catch (_) {
        return new Set();
    }
}

async function render() {
    const library = els.library.value;
    if (!library) {
        els.dashboard.textContent = "";
        els.status.textContent = "No libraries in InfluxDB yet.";
        return;
    }
    const selection = { library, run: els.run.value || ALL, range: els.range.value };
    els.status.textContent = "Loading…";
    els.refresh.disabled = true;

    try {
        const [nesquic, io, quic, qlogs] = await Promise.all([
            points("nesquic", selection),
            points("nesquic_io", selection),
            points("nesquic_quic", selection),
            qlogFiles(library),
        ]);

        charts.length = 0;
        els.dashboard.textContent = "";

        const jobs = experiments.map((e) => e.job);

        const overview = element("section", {}, element("h2", { textContent: "Overview" }));
        const ovGrid = element("div", { className: "grid" });
        overview.append(ovGrid);
        panel(ovGrid, "Throughput With Varying Connection Delay", "Throughput [Mbps]",
            summarize(nesquic.filter((r) => r._field === "throughput"), (r) => r.job, jobs), true);
        els.dashboard.append(overview);

        for (const exp of experiments) {
            const job = exp.job;
            const section = element("section", {},
                element("h2", { textContent: exp.title }),
                element("p", { textContent: exp.description }));
            const grid = element("div", { className: "grid" });
            section.append(grid);

            for (const mode of ["server", "client"]) {
                const rows = io.filter((r) => r.job === job && r.mode === mode);
                panel(grid, `${capitalize(mode)} I/O Syscalls`, "Invocations",
                    summarize(rows.filter((r) => r._field === "count"), (r) => r.syscall));
                panel(grid, `${capitalize(mode)} I/O Data Volume`, "Data Volume [kB]",
                    summarize(rows.filter((r) => r._field === "volume_kb_sum"), (r) => r.syscall));
            }

            const q = quic.filter((r) => r.job === job);
            panel(grid, "ACK Frames Sent", "ACK frames",
                summarize(q.filter((r) => r._field === "acks_sent"), (r) => r.mode, ["server", "client"]));
            panel(grid, "Packets Sent", "Packets",
                summarize(q.filter((r) => r._field === "packets_sent"), (r) => r.mode, ["server", "client"]));

            section.append(qlogSection(library, job, qlogs));
            els.dashboard.append(section);
        }

        redraw();
        els.status.textContent = `Updated ${new Date().toLocaleTimeString()}`;
    } catch (err) {
        console.error(err);
        els.status.textContent = String(err.message || err);
    } finally {
        els.refresh.disabled = false;
    }
}

// --- Controls ---------------------------------------------------------------

function fillSelect(select, values, keep, allLabel) {
    const previous = keep ? select.value : null;
    select.textContent = "";
    if (allLabel) select.append(element("option", { value: ALL, textContent: allLabel }));
    for (const v of values) select.append(element("option", { value: v, textContent: v }));
    if (previous && [...select.options].some((o) => o.value === previous)) select.value = previous;
}

function readHash() {
    const p = new URLSearchParams(location.hash.slice(1));
    return { library: p.get("library"), run: p.get("run"), range: p.get("range") };
}

function writeHash() {
    const p = new URLSearchParams({
        library: els.library.value,
        run: els.run.value,
        range: els.range.value,
    });
    history.replaceState(null, "", `#${p}`);
}

async function loadRuns() {
    const library = els.library.value;
    const runs = library
        ? await tagValues("nesquic_run", `(r) => r.library == ${fluxString(library)}`)
        : [];
    fillSelect(els.run, runs, true, "All");
}

async function init() {
    try {
        const res = await fetch("experiments.yaml", { cache: "no-store" });
        if (!res.ok) throw new Error(`experiments.yaml: HTTP ${res.status}`);
        experiments = yaml.load(await res.text()) || [];

        const wanted = readHash();
        fillSelect(els.library, await tagValues("library"), false);
        if (wanted.library) els.library.value = wanted.library;
        if (wanted.range) els.range.value = wanted.range;
        await loadRuns();
        if (wanted.run) els.run.value = wanted.run;
        if (!els.run.value) els.run.value = ALL;
        if (!els.range.value) els.range.value = "all";
    } catch (err) {
        console.error(err);
        els.status.textContent = String(err.message || err);
        return;
    }

    els.library.addEventListener("change", async () => {
        await loadRuns();
        writeHash();
        render();
    });
    for (const s of [els.run, els.range]) {
        s.addEventListener("change", () => {
            writeHash();
            render();
        });
    }
    els.refresh.addEventListener("click", render);

    let pending = 0;
    window.addEventListener("resize", () => {
        cancelAnimationFrame(pending);
        pending = requestAnimationFrame(redraw);
    });

    writeHash();
    render();
}

init();
