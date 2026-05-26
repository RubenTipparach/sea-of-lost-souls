import * as THREE from 'three';
import './style.css';
import { defaultShipDef, serializeShipDef } from './shipDef';
import type { Hardpoint, Subsystem } from './shipDef';
import { SceneManager } from './scene';
import type { Marker, MarkerKind } from './scene';
import { Panel } from './panel';
import { el, downloadBlob } from './dom';

const viewport = document.getElementById('viewport') as HTMLElement;
const panelHost = document.getElementById('panel') as HTMLElement;

const def = defaultShipDef();

const scene = new SceneManager(viewport, {
  onMarkerMoved: (marker, pos) => writeMarkerPosition(marker, pos),
  onSelectionChanged: (marker) => syncPanelSelection(marker),
});

const panel = new Panel(panelHost, def, {
  onChanged: () => {},
  onSelectHardpoint: (id) => selectMarker('hardpoint', id),
  onAddHardpoint: () => addHardpoint(),
  onRemoveHardpoint: (id) => removeEntry('hardpoint', id),
  onRenameHardpoint: (oldId, newId) => scene.renameMarker('hardpoint', oldId, newId),
  onSelectSubsystem: (id) => selectMarker('subsystem', id),
  onAddSubsystem: () => addSubsystem(),
  onRemoveSubsystem: (id) => removeEntry('subsystem', id),
  onRenameSubsystem: (oldId, newId) => scene.renameMarker('subsystem', oldId, newId),
});
panel.render();

buildToolbar();
spawnInitialMarkers();

// ---- Marker <-> ShipDef wiring --------------------------------------------

function vecToArray(v: THREE.Vector3): [number, number, number] {
  // Round to keep ship.json tidy; positions feed glTF node transforms.
  const r = (n: number) => Math.round(n * 1000) / 1000;
  return [r(v.x), r(v.y), r(v.z)];
}

// Editor-only authoring positions, keyed by node name. These are NOT part of
// the strict ShipDef schema; on export they belong as glTF node transforms.
// TODO: write these onto named anchor nodes in the exported GLB.
const anchorPositions = new Map<string, [number, number, number]>();

function writeMarkerPosition(marker: Marker, pos: THREE.Vector3): void {
  const node = nodeForRef(marker.kind, marker.refId);
  if (node) anchorPositions.set(node, vecToArray(pos));
}

function nodeForRef(kind: MarkerKind, refId: string): string | null {
  if (kind === 'hardpoint') {
    return def.hardpoints.find((h) => h.id === refId)?.node ?? null;
  }
  return def.powerGrid.subsystems.find((s) => s.id === refId)?.node ?? null;
}

function spawnInitialMarkers(): void {
  let i = 0;
  for (const hp of def.hardpoints) {
    scene.addMarker('hardpoint', hp.id, new THREE.Vector3(0, 0.6 + i * 0.8, 2));
    i++;
  }
  i = 0;
  for (const sub of def.powerGrid.subsystems) {
    scene.addMarker('subsystem', sub.id, new THREE.Vector3(-1.5, 0.6 + i * 0.8, -1));
    i++;
  }
}

function selectMarker(kind: MarkerKind, id: string): void {
  // Create a marker at origin if the entry was added without one yet.
  const marker =
    scene.findMarker(kind, id) ?? scene.addMarker(kind, id, new THREE.Vector3(0, 0.6, 0));
  scene.select(marker);
}

function syncPanelSelection(marker: Marker | null): void {
  if (!marker) {
    panel.setSelectedHardpoint(null);
    panel.setSelectedSubsystem(null);
    return;
  }
  if (marker.kind === 'hardpoint') panel.setSelectedHardpoint(marker.refId);
  else panel.setSelectedSubsystem(marker.refId);
}

function uniqueId(base: string, taken: Set<string>): string {
  if (!taken.has(base)) return base;
  let n = 2;
  while (taken.has(`${base}_${n}`)) n++;
  return `${base}_${n}`;
}

function addHardpoint(): void {
  const taken = new Set(def.hardpoints.map((h) => h.id));
  const id = uniqueId('hardpoint', taken);
  const hp: Hardpoint = {
    id,
    node: `hp_${id}`,
    mount: 'fixed',
    weapon: 'pulse_cannon',
    arcDeg: 10,
    drawPerShot: 4,
  };
  def.hardpoints.push(hp);
  const marker = scene.addMarker('hardpoint', id, new THREE.Vector3(0, 0.6, 2));
  writeMarkerPosition(marker, marker.object.position);
  panel.render();
  scene.select(marker);
}

function addSubsystem(): void {
  const taken = new Set(def.powerGrid.subsystems.map((s) => s.id));
  const id = uniqueId('subsystem', taken);
  const sub: Subsystem = { id, node: `${id}_mount`, draw: 1, priority: 1 };
  def.powerGrid.subsystems.push(sub);
  const marker = scene.addMarker('subsystem', id, new THREE.Vector3(-1.5, 0.6, -1));
  writeMarkerPosition(marker, marker.object.position);
  panel.render();
  scene.select(marker);
}

function removeEntry(kind: MarkerKind, id: string): void {
  if (kind === 'hardpoint') {
    def.hardpoints = def.hardpoints.filter((h) => h.id !== id);
  } else {
    def.powerGrid.subsystems = def.powerGrid.subsystems.filter((s) => s.id !== id);
  }
  scene.removeMarker(kind, id);
  panel.render();
}

// ---- Toolbar (viewport overlay) -------------------------------------------

function buildToolbar(): void {
  const importInput = el('input', {
    type: 'file',
    accept: '.glb,.gltf',
    style: 'display:none',
  });
  importInput.addEventListener('change', async () => {
    const file = importInput.files?.[0];
    if (!file) return;
    try {
      await scene.importGLB(file);
    } catch (err) {
      console.error('GLB import failed', err);
      alert(`GLB import failed: ${String(err)}`);
    }
    importInput.value = '';
  });

  const importBtn = el('button', {}, ['Import GLB']);
  importBtn.addEventListener('click', () => importInput.click());

  const colliderToggle = el('input', { type: 'checkbox' });
  colliderToggle.addEventListener('change', () => {
    scene.setColliderVisible(colliderToggle.checked);
  });
  const colliderLabel = el('label', { class: 'checkbox' }, [colliderToggle, 'Collider']);

  const exportGlbBtn = el('button', {}, ['Export GLB']);
  exportGlbBtn.addEventListener('click', () => void exportGLB());

  const exportJsonBtn = el('button', {}, ['Export JSON']);
  exportJsonBtn.addEventListener('click', () => exportJSON());

  const exportBothBtn = el('button', { class: 'primary' }, ['Export Both']);
  exportBothBtn.addEventListener('click', () => void exportBoth());

  const toolbar = el('div', { class: 'toolbar' }, [
    importBtn,
    colliderLabel,
    exportGlbBtn,
    exportJsonBtn,
    exportBothBtn,
    importInput,
  ]);
  viewport.appendChild(toolbar);
}

// ---- Export ----------------------------------------------------------------

async function exportGLB(): Promise<void> {
  const buffer = await scene.exportGLB();
  downloadBlob('model.glb', new Blob([buffer], { type: 'model/gltf-binary' }));
}

function exportJSON(): void {
  def.model = 'model.glb';
  const json = serializeShipDef(def);
  downloadBlob('ship.json', new Blob([json], { type: 'application/json' }));
}

async function exportBoth(): Promise<void> {
  await exportGLB();
  exportJSON();
}
