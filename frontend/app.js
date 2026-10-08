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
    // Not schema.tagValues(): InfluxDB's index keeps listing values whose
    // points were all deleted (script/run.sh deletes earlier results).
    // first() forces a read of the data itself.
    const lines = [`from(bucket: ${fluxString(BUCKET)})`, `  |> range(start: ${EPOCH})`];
    if (predicate) lines.push(`  |> filter(fn: ${predicate})`);
    lines.push(
        "  |> first()",
        `  |> keep(columns: [${fluxString(tag)}])`,
        "  |> group()",
        `  |> distinct(column: ${fluxString(tag)})`,
    );
    const rows = await flux(lines.join("\n"));
    return rows.map((r) => r._value).filter(Boolean).sort(d3.ascending);
}

/** All points of `measurement` for the selected library (or all), run and time range. */
async function points(measurement, { library, run, range }) {
    const start = range === "all" ? EPOCH : range;
    const lines = [
        `from(bucket: ${fluxString(BUCKET)})`,
        `  |> range(start: ${start})`,
        `  |> filter(fn: (r) => r._measurement == ${fluxString(measurement)})`,
    ];
    if (library !== ALL) {
        lines.push(`  |> filter(fn: (r) => r.library == ${fluxString(library)})`);
    }
    if (run !== ALL) {
        lines.push(`  |> filter(fn: (r) => r.nesquic_run == ${fluxString(run)})`);
    }
    lines.push(
        "  |> toFloat()",
        '  |> keep(columns: ["_time", "_field", "_value", "job", "library", "mode", "syscall", "nesquic_run"])',
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
                // Only the rows of latencyParts() are split into parts.
                parts: v[0].parts && v[0].parts.map((_, i) => d3.mean(v, (r) => r.parts[i])),
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

/** What a request spends its latency on; the segments of a stacked bar. */
const LATENCY_PARTS = ["Crypto", "I/O", "Other"];

/**
 * One row per client process with its mean request latency as `_value`, split
 * into `parts` (see LATENCY_PARTS): the time the process spends in crypto and
 * in I/O while a request is outstanding, and the rest of the latency.
 *
 * That is the process' duration per request, times the number of requests
 * outstanding at once: they overlap, so the same crypto or I/O time is part
 * of the latency of each of them.
 */
function latencyParts(rows) {
    // A process reports all its fields in one point.
    const processes = d3.group(rows, (r) => [r.library, r.job, r.nesquic_run, r._time].join("\0"));
    const out = [];
    for (const fields of processes.values()) {
        const f = Object.fromEntries(fields.map((r) => [r._field, r._value]));
        if (!(f.requests > 0) || f.request_latency_ms === undefined) continue;
        const latency = f.request_latency_ms;
        // Mean number of outstanding requests; 1 for a single request.
        const concurrent = f.request_window_ms > 0 ? f.requests * latency / f.request_window_ms : 1;
        const perRequest = (duration) => (duration || 0) / f.requests * concurrent;
        // Durations of several threads, or with the handshake, may exceed the latency.
        const crypto = Math.min(latency, perRequest(f.crypto_duration_ms));
        const io = Math.min(latency - crypto, perRequest(f.io_duration_ms));
        out.push({ ...fields[0], _value: latency, parts: [crypto, io, latency - crypto - io] });
    }
    return out;
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
        ...(d.parts || []).map((value, i) => [LATENCY_PARTS[i], fmt(value)]),
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

    // Bars: 4px rounded top, square at the baseline. A stacked bar is one
    // segment per part, separated by a 2px gap; only the topmost is rounded.
    const r = Math.min(4, x.bandwidth() / 2);
    const segments = (d) => {
        let lo = 0;
        const parts = (d.parts || [d.mean]).map((value, part) => ({ d, part, lo, hi: (lo += value) }));
        return parts.filter((s) => s.hi > s.lo).map((s, i, all) => ({ ...s, top: i === all.length - 1 }));
    };
    const barPath = ({ d, lo, hi, top }) => {
        const x0 = x(d.category);
        const x1 = x0 + x.bandwidth();
        const y0 = y(lo);
        const y1 = Math.min(y0, y(hi) + (top ? 0 : 2));
        const rr = top ? Math.min(r, y0 - y1) : 0;
        return `M${x0},${y0}V${y1 + rr}Q${x0},${y1} ${x0 + rr},${y1}` +
            `H${x1 - rr}Q${x1},${y1} ${x1},${y1 + rr}V${y0}Z`;
    };

    const bar = g.append("g").selectAll("g").data(data).join("g");

    bar.selectAll("path").data(segments).join("path")
        .attr("class", (s) => `bar part-${s.part}`)
        .attr("d", barPath);

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
            d3.select(this.parentNode).selectAll(".bar").classed("hover", true);
            showTooltip(event, d, yLabel);
        })
        .on("pointerleave", function () {
            d3.select(this.parentNode).selectAll(".bar").classed("hover", false);
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
    // Stacked bars need a legend for their parts.
    const legend = element("div", { className: "legend" },
        ...(data.some((d) => d.parts) ? LATENCY_PARTS : []).map((name, i) =>
            element("span", {}, element("i", { className: `part-${i}` }), name)));
    parent.append(element("div", { className: wide ? "panel wide" : "panel" },
        element("h3", { textContent: title }), legend, chart));
    charts.push({ chart, data, yLabel });
}

function redraw() {
    for (const c of charts) drawBarChart(c.chart, c.data, c.yLabel);
}

function capitalize(s) {
    return s.charAt(0).toUpperCase() + s.slice(1);
}

function qlogSection(library, job, side, available) {
    const dir = `qlog/${encodeURIComponent(library)}/`;
    const stem = `${job}.${side}`;
    const head = element("div", { className: "head" },
        element("h3", { textContent: `${capitalize(side)} qlog` }));
    const box = element("div", { className: "qlog" }, head);

    if (!available.has(`${stem}.html`)) {
        head.append(element("span", {
            className: "muted",
            textContent: `No rendered trace at res/qlog/${library}/${stem}.html.`,
        }));
        return box;
    }

    const page = dir + encodeURIComponent(`${stem}.html`);
    head.append(
        element("a", { href: page, target: "_blank", textContent: "Open in new tab" }),
        element("a", { href: dir + encodeURIComponent(`${stem}.qlog`), download: "", textContent: "Download qlog" }));
    const frame = element("iframe", { src: page, title: `qvis: ${library} ${job} ${side}`, loading: "lazy" });
    frame.addEventListener("load", () => frame.classList.add("loaded"));
    box.append(frame);
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

/** Per experiment, throughput and request latency of every library. */
async function renderOverview(selection) {
    const [nesquic, latency] = await Promise.all([
        points("nesquic", selection),
        points("nesquic_latency", selection).then(latencyParts),
    ]);

    charts.length = 0;
    els.dashboard.textContent = "";

    for (const exp of experiments) {
        const rows = nesquic.filter((r) => r.job === exp.job);
        const section = element("section", {},
            element("h2", { textContent: exp.title }),
            element("p", { textContent: exp.description }));
        const grid = element("div", { className: "grid" });
        section.append(grid);
        panel(grid, "Throughput", "Throughput [Mbps]",
            summarize(rows.filter((r) => r._field === "throughput"), (r) => r.library), true);
        panel(grid, "Request Latency", "Mean latency [ms]",
            summarize(latency.filter((r) => r.job === exp.job), (r) => r.library), true);
        els.dashboard.append(section);
    }
}

async function renderLibrary(selection) {
    const { library } = selection;
    const [nesquic, latency, io, quic, qlogs] = await Promise.all([
        points("nesquic", selection),
        points("nesquic_latency", selection).then(latencyParts),
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
    panel(ovGrid, "Request Latency", "Mean latency [ms]",
        summarize(latency, (r) => r.job, jobs), true);
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

        for (const side of ["server", "client"]) {
            section.append(qlogSection(library, job, side, qlogs));
        }
        els.dashboard.append(section);
    }
}

async function render() {
    const library = els.library.value;
    if (els.library.options.length < 2) {
        els.dashboard.textContent = "";
        els.status.textContent = "No libraries in InfluxDB yet.";
        return;
    }
    const selection = { library, run: els.run.value || ALL, range: els.range.value };
    els.status.textContent = "Loading…";
    els.refresh.disabled = true;

    try {
        await (library === ALL ? renderOverview : renderLibrary)(selection);
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
    const runs = await tagValues("nesquic_run",
        library === ALL ? null : `(r) => r.library == ${fluxString(library)}`);
    fillSelect(els.run, runs, true, "All");
}

async function init() {
    try {
        const res = await fetch("experiments.yaml", { cache: "no-store" });
        if (!res.ok) throw new Error(`experiments.yaml: HTTP ${res.status}`);
        experiments = yaml.load(await res.text()) || [];

        const wanted = readHash();
        fillSelect(els.library, await tagValues("library"), false, "Overview");
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
