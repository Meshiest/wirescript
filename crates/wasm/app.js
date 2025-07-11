// Wasm
import initBearilog from './pkg/wasm.js';
import * as bearilog from './pkg/wasm_bg.js';

// Graphviz
import 'https://cdn.jsdelivr.net/npm/@viz-js/viz@3.14.0/lib/viz-standalone.min.js';

// Preact
import htm from 'https://esm.sh/htm';
import { h, render } from 'https://esm.sh/preact';
import { useEffect, useRef, useState } from 'https://esm.sh/preact/hooks';

// Code editor
import * as monaco from 'https://cdn.jsdelivr.net/npm/monaco-editor@0.52.2/+esm';

// Svg panning
import { initializeSvgToolbelt } from 'https://unpkg.com/svg-toolbelt@latest/dist/svg-toolbelt.esm.js';

let viz;

// Initialize htm with Preact
const html = htm.bind(h);

const debounce = (func, delay) => {
  let timeout;
  return (...args) => {
    clearTimeout(timeout);
    timeout = setTimeout(() => {
      func.apply(this, args);
    }, delay);
  };
};

let initialCode =
  localStorage.getItem('bearilog_code') ||
  `
# Bearilog modules can have multiple inputs and outputs.
module hello(a, b) -> sum, product {
  # There is no need to return values, as their
  # variable names are defined by the outputs
  sum = a + b;
  product = a * b;

  # Define new variables
  let result = sum / product;
}

# Call modules from other modules
# (Force inline some modules to save space)
inline module goodbye(x) -> y {
  y, _ = hello(x, x);

  # Use a wide range of operators
  const z = 1 & 2 | 3 ^ 4 && 5 ^^ 6 || 7 << 8 >> 9 % 10 / 11 * 12 + 13 - 14;
  # Or comparisons
  const w = 1 == 2 >= 3 <= 4 != 5;

  # Use edge detectors and the blend gate
  const up, down = edge(z);
  const blended = blend(up, down, 0.5);
}
`;

function Checkbox({ id, label, checked, onChange }) {
  return html`
    <span class="checkbox">
      <input
        type="checkbox"
        id=${id}
        checked=${checked}
        onchange=${e => onChange(e.target.checked)}
      />
      <label for=${id}>${label}</label>
    </span>
  `;
}

function Numberbox({ id, label, value, onChange, min, max }) {
  return html`
    <div class="textbox">
      <label for=${id}>${label}</label>
      <input
        type="number"
        min=${min || ''}
        max=${max || ''}
        step="1"
        id=${id}
        value=${value}
        oninput=${e => onChange(e.target.value)}
      />
    </div>
  `;
}

function App(props) {
  const [loading, setLoading] = useState(true);
  const [bearilogReady, setBearilogReady] = useState(false);
  const [loadError, setLoadError] = useState(null);
  const [editor, setEditor] = useState(null);
  const [code, setCode] = useState(initialCode);

  const [bearilogError, setBearilogError] = useState(null);
  const [bearilogModules, setBearilogModules] = useState([]);
  const [selectedModule, setSelectedModule] = useState(null);
  const [graphvizError, setGraphvizError] = useState(null);
  const containerRef = useRef(null);
  const [inline, setInline] = useState(false);
  const [downloadOk, setDownloadOk] = useState(null);
  const clearDownloadRef = useRef(null);

  const [showMore, setShowMore] = useState(false);

  const [gridMode, setGridMode] = useState(false);

  // layout options:
  // gapW, gapH, margin, padding, indent, flat
  // grid options:
  // width (non-zero), height (non-zero), layerMode, iobelow

  const [gapW, setGapW] = useState(0);
  const [gapH, setGapH] = useState(0);
  const [margin, setMargin] = useState(0);
  const [padding, setPadding] = useState(0);
  const [indent, setIndent] = useState(0);
  const [flat, setFlat] = useState(false);

  const [width, setWidth] = useState(8);
  const [height, setHeight] = useState(8);
  const [layerMode, setLayerMode] = useState(false);
  const [iobelow, setIobelow] = useState(false);

  useEffect(() => {
    Promise.all([initBearilog(), Viz.instance()])
      .then(([b, v]) => {
        viz = v;
        bearilog.__wbg_set_wasm(b);
        setBearilogReady(true);

        console.log('Bearilog and Graphviz initialized');

        const editor = monaco.editor.create(document.getElementById('monaco'), {
          value: initialCode,
          language: 'ruby',
          theme: 'vs-dark',
          automaticLayout: true,
        });
        const setCodeDebounced = debounce(setCode, 500);

        editor.onKeyDown(() => {
          setBearilogError(null);
        });
        editor.onKeyUp(() => {
          const code = editor.getValue();
          setCodeDebounced(code);
          localStorage.setItem('bearilog_code', code);
        });

        setEditor(editor);
      })
      .catch(err => {
        console.error('Error initializing Bearilog or Graphviz:', err);
        setLoadError(err);
      })
      .finally(() => {
        setLoading(false);
      });
  }, []);

  useEffect(() => {
    if (!bearilogReady || !code) return;

    try {
      const next = bearilog.get_modules(code);
      setBearilogModules(prev => {
        // If there are new modules, update the list
        if (
          prev.length !== next.length ||
          !prev.every((v, i) => v === next[i])
        ) {
          return next;
        }
        return prev;
      });
      setBearilogError(null);
    } catch (err) {
      console.error('Error getting modules:', err);
      setBearilogError(err);
    }
  }, [code, bearilogReady]);

  useEffect(() => {
    setSelectedModule(m => {
      // Select the first module if none is selected
      if (!m && bearilogModules.length > 0) {
        return bearilogModules[0];
      }
      // Deselect the module if it is not in the list
      if (m && !bearilogModules.includes(m)) {
        return null;
      }
      return m;
    });
  }, [Boolean(selectedModule), bearilogModules]);

  useEffect(() => {
    if (!bearilogReady || !selectedModule || !code) return;
    try {
      const graph = bearilog
        .graphviz(code, selectedModule, inline)
        .replace(/b?Input(A|B)/gi, '$1')
        .replace(/bPulseOn(Rising|Falling)Edge/gi, '$1');
      const svg = viz.renderSVGElement(graph);
      setGraphvizError(null);

      const svgs = containerRef.current.querySelector('.svgs');
      if (svgs.firstChild) {
        // Super forbidden way to replace the SVG Content
        svgs.firstChild.innerHTML = svg.innerHTML;
        svgs.firstChild.setAttribute('viewBox', svg.getAttribute('viewBox'));
        svgs.firstChild.setAttribute('width', svg.getAttribute('width'));
        svgs.firstChild.setAttribute('height', svg.getAttribute('height'));
      } else {
        // Add the svg to the container
        svgs.appendChild(svg);
        // Initialize SVG toolbelt for panning and zooming
        initializeSvgToolbelt(svgs, {
          controlsPosition: 'bottom-right',
        });
      }
    } catch (err) {
      setGraphvizError(err.toString());
      console.error('Error generating graph:', err);
    }
  }, [inline, selectedModule, code, bearilogReady]);

  const download = () => {
    if (!bearilogReady || !selectedModule || !code) return;
    clearTimeout(clearDownloadRef.current);
    setDownloadOk(null);

    try {
      const buf = gridMode
        ? bearilog.grid(code, selectedModule, {
            inline,
            width,
            height,
            layerMode,
            iobelow,
          })
        : bearilog.layout(code, selectedModule, {
            inline,
            gapW,
            gapH,
            margin,
            padding,
            indent,
            flat,
          });

      // Click and download the Bearilog file
      const blob = new Blob([buf], { type: 'application/octet-stream' });
      const url = URL.createObjectURL(blob);
      const a = document.createElement('a');
      a.href = url;
      a.download = `${selectedModule}.brz`;
      document.body.appendChild(a);
      a.click();
      document.body.removeChild(a);
      URL.revokeObjectURL(url);
      setDownloadOk(true);
    } catch (err) {
      console.error('Error generating Bearilog file:', err);
      setDownloadOk(false);
    }

    // Clear the download status after 2 seconds
    clearDownloadRef.current = setTimeout(() => {
      setDownloadOk(null);
    }, 2000);
  };

  let overlay = null;
  if (loading) overlay = html`<div class="overlay loading">Loading...</div>`;
  else if (loadError)
    overlay = html`<div class="overlay">
      <div class="error">
        <b>Load Error</b>
        <p>${loadError.toString()}</p>
      </div>
    </div>`;

  return html`
    <div id="monaco"></div>
    <div class="controls">
      <select onchange=${e => setSelectedModule(e.target.value)}>
        <option value="" disabled selected>Select Module</option>
        ${bearilogModules.map(
          m => html`<option value=${m} selected=${m === selectedModule}>
            ${m}
          </option>`
        )}
      </select>
      <button
        disabled=${!bearilogReady || !selectedModule || !code}
        onclick=${() => download()}
      >
        ${downloadOk === true
          ? 'Downloaded!'
          : downloadOk === false
          ? 'Error!'
          : `Download ${selectedModule}.brz`}
      </button>
      <${Checkbox}
        id="inline"
        label="Inline"
        checked=${inline}
        onChange=${setInline}
      />
      <div class="settings">
        <button onclick=${() => setShowMore(s => !s)}>Render Settings</button>
        ${showMore &&
        html`<div class="more-settings">
          <${Checkbox}
            id="uglyMode"
            label="Compact Mode"
            checked=${gridMode}
            onChange=${setGridMode}
          />
          ${!gridMode &&
          html`
            <${Numberbox}
              id="gapW"
              label="Gap Width"
              value=${gapW}
              onChange=${setGapW}
            />
            <${Numberbox}
              id="gapH"
              label="Gap Height"
              value=${gapH}
              onChange=${setGapH}
            />
            <${Numberbox}
              id="margin"
              label="Margin"
              value=${margin}
              onChange=${setMargin}
            />
            <${Numberbox}
              id="padding"
              label="Padding"
              value=${padding}
              onChange=${setPadding}
            />
            <${Numberbox}
              id="indent"
              label="Indent"
              value=${indent}
              onChange=${setIndent}
            />
            <${Checkbox}
              id="flat"
              label="Flat Mode"
              checked=${flat}
              onChange=${setFlat}
            />
          `}
          ${gridMode &&
          html`
            <${Numberbox}
              id="width"
              label="Num Columns"
              value=${width}
              onChange=${setWidth}
              min="1"
            />
            <${Numberbox}
              id="height"
              label=${layerMode ? 'Num Rows' : 'Column Height'}
              value=${height}
              onChange=${setHeight}
              min="1"
            />
            <${Checkbox}
              id="layerMode"
              label="Layer Mode"
              checked=${layerMode}
              onChange=${setLayerMode}
            />
            <${Checkbox}
              id="iobelow"
              label="IO Below"
              checked=${iobelow}
              onChange=${setIobelow}
            />
          `}
        </div>`}
      </div>

      ${graphvizError &&
      html`<div class="graphviz-error">${graphvizError.toString()}</div>`}
    </div>
    <div class="graphviz-container" ref=${containerRef}>
      <div class="svgs"></div>
    </div>
    ${overlay}
  `;
}

render(html`<${App} name="World" />`, document.body);
