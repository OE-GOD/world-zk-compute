// ZKML Verifier Demo — Browser Application
//
// Loads the WASM verifier and provides a UI for verifying GKR+Hyrax proofs.

// ============================================================
// State
// ============================================================

let wasmReady = false;
let wasmModule = null;   // { verify_proof_json, version }
let currentBundle = null; // raw JSON string of the loaded proof bundle

// ============================================================
// DOM references
// ============================================================

const $ = (id) => document.getElementById(id);

const els = {
    // Header
    versionBadge:     $('version-badge'),
    themeToggle:      $('theme-toggle'),
    themeIcon:        $('theme-icon'),

    // Bundle section
    loadSampleBtn:    $('load-sample-btn'),
    uploadInput:      $('upload-input'),
    bundleStatus:     $('bundle-status'),
    bundleStatusText: $('bundle-status-text'),
    bundleCircuitHash:$('bundle-circuit-hash'),
    bundleProofSize:  $('bundle-proof-size'),
    bundleGensSize:   $('bundle-gens-size'),
    bundleLayers:     $('bundle-layers'),
    bundleInputs:     $('bundle-inputs'),

    // Verify section
    verifyBtn:        $('verify-btn'),
    verifyResult:     $('verify-result'),
    resultBanner:     $('result-banner'),
    resultIcon:       $('result-icon'),
    resultText:       $('result-text'),
    resultCircuitHash:$('result-circuit-hash'),
    resultTime:       $('result-time'),
    resultErrorRow:   $('result-error-row'),
    resultError:      $('result-error'),

    // Raw section
    rawToggle:        $('raw-toggle'),
    rawContent:       $('raw-content'),
    rawSection:       $('raw-section'),
    rawProofHex:      $('raw-proof-hex'),
    rawCircuitDesc:   $('raw-circuit-desc'),

    // Overlay
    loadingOverlay:   $('loading-overlay'),
    loadingText:      $('loading-text'),
};

// ============================================================
// Theme
// ============================================================

function initTheme() {
    const saved = localStorage.getItem('zkml-theme');
    const prefersDark = window.matchMedia('(prefers-color-scheme: dark)').matches;
    const theme = saved || (prefersDark ? 'dark' : 'light');
    setTheme(theme);
}

function setTheme(theme) {
    document.documentElement.setAttribute('data-theme', theme);
    localStorage.setItem('zkml-theme', theme);
    // Moon for dark, sun for light
    els.themeIcon.textContent = theme === 'dark' ? '\u2600' : '\u263E';
}

function toggleTheme() {
    const current = document.documentElement.getAttribute('data-theme');
    setTheme(current === 'dark' ? 'light' : 'dark');
}

// ============================================================
// WASM Initialization
// ============================================================

async function initWasm() {
    els.loadingOverlay.classList.remove('hidden');
    els.loadingText.textContent = 'Loading WASM verifier...';

    try {
        // Dynamic import so the page works even if pkg/ is missing
        const mod = await import('./pkg/zkml_verifier.js');
        await mod.default();  // init()
        wasmModule = mod;
        wasmReady = true;

        // Show version badge
        const ver = mod.version();
        els.versionBadge.textContent = 'v' + ver;
        els.versionBadge.classList.remove('hidden');

        console.log('[zkml] WASM verifier loaded, version:', ver);
    } catch (err) {
        console.warn('[zkml] WASM module not available:', err.message);
        console.log('[zkml] Running in demo mode (verification disabled)');
        wasmReady = false;
    }

    els.loadingOverlay.classList.add('hidden');
}

// ============================================================
// Bundle Loading
// ============================================================

function formatBytes(bytes) {
    if (bytes < 1024) return bytes + ' B';
    if (bytes < 1024 * 1024) return (bytes / 1024).toFixed(1) + ' KB';
    return (bytes / (1024 * 1024)).toFixed(2) + ' MB';
}

function hexByteLength(hexStr) {
    const stripped = hexStr.startsWith('0x') ? hexStr.slice(2) : hexStr;
    return Math.floor(stripped.length / 2);
}

function displayBundle(bundle) {
    const desc = bundle.dag_circuit_description || {};

    els.bundleStatusText.textContent = 'Loaded';
    els.bundleStatusText.style.color = 'var(--success)';

    // Circuit hash from the bundle metadata or proof header
    if (bundle.circuit_hash) {
        els.bundleCircuitHash.textContent = bundle.circuit_hash;
    } else {
        // Extract from proof_hex: bytes 4..36 (after REM1 selector)
        const stripped = bundle.proof_hex.startsWith('0x')
            ? bundle.proof_hex.slice(2)
            : bundle.proof_hex;
        const hashHex = stripped.slice(8, 8 + 64); // 4 bytes selector = 8 hex chars
        els.bundleCircuitHash.textContent = '0x' + hashHex;
    }

    els.bundleProofSize.textContent = formatBytes(hexByteLength(bundle.proof_hex));
    els.bundleGensSize.textContent = formatBytes(hexByteLength(bundle.gens_hex));
    els.bundleLayers.textContent = desc.numComputeLayers || '--';
    els.bundleInputs.textContent = desc.numInputLayers || '--';

    els.bundleStatus.classList.remove('hidden');

    // Raw data
    const stripped = bundle.proof_hex.startsWith('0x')
        ? bundle.proof_hex.slice(2)
        : bundle.proof_hex;
    els.rawProofHex.textContent = stripped.slice(0, 256) + '...';
    els.rawCircuitDesc.textContent = JSON.stringify(desc, null, 2).slice(0, 2000);

    // Enable verify button
    els.verifyBtn.disabled = false;
    els.verifyResult.classList.add('hidden');
}

async function loadSampleBundle() {
    els.bundleStatusText.textContent = 'Loading...';
    els.bundleStatusText.style.color = 'var(--text)';
    els.bundleStatus.classList.remove('hidden');

    try {
        const resp = await fetch('sample_proof.json');
        if (!resp.ok) throw new Error('HTTP ' + resp.status);
        const text = await resp.text();
        currentBundle = text;
        const bundle = JSON.parse(text);
        displayBundle(bundle);
    } catch (err) {
        els.bundleStatusText.textContent = 'Failed to load: ' + err.message;
        els.bundleStatusText.style.color = 'var(--error)';
    }
}

function handleUpload(event) {
    const file = event.target.files[0];
    if (!file) return;

    const reader = new FileReader();
    reader.onload = (e) => {
        try {
            const text = e.target.result;
            const bundle = JSON.parse(text);

            // Validate required fields
            if (!bundle.proof_hex || !bundle.gens_hex || !bundle.dag_circuit_description) {
                throw new Error('Missing required fields: proof_hex, gens_hex, dag_circuit_description');
            }

            currentBundle = text;
            displayBundle(bundle);
        } catch (err) {
            els.bundleStatusText.textContent = 'Invalid bundle: ' + err.message;
            els.bundleStatusText.style.color = 'var(--error)';
            els.bundleStatus.classList.remove('hidden');
        }
    };
    reader.readAsText(file);
}

// ============================================================
// Verification
// ============================================================

function runVerification() {
    if (!currentBundle) {
        alert('Load a proof bundle first.');
        return;
    }

    // Reset result UI
    els.verifyResult.classList.add('hidden');
    els.resultErrorRow.classList.add('hidden');

    if (!wasmReady) {
        // Demo mode: show a simulated result with a warning
        showDemoResult();
        return;
    }

    // Show spinner
    els.verifyBtn.disabled = true;
    els.verifyBtn.textContent = 'Verifying...';

    // Use requestAnimationFrame to let the UI update before heavy computation
    requestAnimationFrame(() => {
        setTimeout(() => {
            const t0 = performance.now();
            let result;
            try {
                result = wasmModule.verify_proof_json(currentBundle);
            } catch (err) {
                result = { verified: false, circuit_hash: '', error: err.message };
            }
            const elapsed = performance.now() - t0;

            showResult(result.verified, result.circuit_hash, elapsed, result.error);

            els.verifyBtn.disabled = false;
            els.verifyBtn.textContent = 'Verify Proof';
        }, 50); // Small delay to allow UI repaint
    });
}

function showResult(verified, circuitHash, timeMs, error) {
    els.verifyResult.classList.remove('hidden');

    if (verified) {
        els.resultBanner.className = 'result-banner success';
        els.resultIcon.textContent = '\u2713';
        els.resultText.textContent = 'VERIFIED';
    } else {
        els.resultBanner.className = 'result-banner failure';
        els.resultIcon.textContent = '\u2717';
        els.resultText.textContent = 'VERIFICATION FAILED';
    }

    els.resultCircuitHash.textContent = circuitHash || '--';
    els.resultTime.textContent = timeMs.toFixed(1) + ' ms';

    if (error) {
        els.resultErrorRow.classList.remove('hidden');
        els.resultError.textContent = error;
    } else {
        els.resultErrorRow.classList.add('hidden');
    }
}

function showDemoResult() {
    // When WASM is not available, show an informational message
    els.verifyResult.classList.remove('hidden');
    els.resultBanner.className = 'result-banner failure';
    els.resultIcon.textContent = '!';
    els.resultText.textContent = 'WASM NOT AVAILABLE';
    els.resultCircuitHash.textContent = '--';
    els.resultTime.textContent = '--';
    els.resultErrorRow.classList.remove('hidden');
    els.resultError.textContent =
        'The WASM verifier module is not loaded. ' +
        'Build it with: wasm-pack build --target web crates/zkml-verifier -- --features wasm ' +
        'then copy pkg/ to web/pkg/';
}

// ============================================================
// Collapsible Sections
// ============================================================

function initCollapsible() {
    els.rawToggle.addEventListener('click', () => {
        const section = els.rawSection;
        const content = els.rawContent;
        section.classList.toggle('collapsed');
        content.classList.toggle('hidden');
    });
}

// ============================================================
// Init
// ============================================================

async function main() {
    initTheme();
    initCollapsible();

    // Event listeners
    els.themeToggle.addEventListener('click', toggleTheme);
    els.loadSampleBtn.addEventListener('click', loadSampleBundle);
    els.uploadInput.addEventListener('change', handleUpload);
    els.verifyBtn.addEventListener('click', runVerification);

    // Load WASM
    await initWasm();
}

main();
