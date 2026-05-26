import * as THREE from 'three';
import { OrbitControls } from 'three/addons/controls/OrbitControls.js';
import { TransformControls } from 'three/addons/controls/TransformControls.js';
import { GLTFLoader } from 'three/addons/loaders/GLTFLoader.js';
import { GLTFExporter } from 'three/addons/exporters/GLTFExporter.js';

export type MarkerKind = 'hardpoint' | 'subsystem';

export interface Marker {
  kind: MarkerKind;
  /** Stable id used to map back into the ShipDef entry. */
  refId: string;
  object: THREE.Object3D;
}

export interface SceneCallbacks {
  /** Fired while dragging a marker; payload position is in world space. */
  onMarkerMoved?: (marker: Marker, position: THREE.Vector3) => void;
  /** Fired when selection changes (null when nothing is selected). */
  onSelectionChanged?: (marker: Marker | null) => void;
}

const HARDPOINT_COLOR = 0xff7043;
const SUBSYSTEM_COLOR = 0x4ea1ff;

/**
 * Owns the Three.js viewport: renderer, camera, lights, helpers, the ship mesh,
 * editable anchor markers, the collider wireframe, and GLB import/export.
 */
export class SceneManager {
  readonly scene = new THREE.Scene();
  readonly camera: THREE.PerspectiveCamera;
  readonly renderer: THREE.WebGLRenderer;

  private readonly orbit: OrbitControls;
  private readonly transform: TransformControls;
  private readonly raycaster = new THREE.Raycaster();
  private readonly pointer = new THREE.Vector2();

  private readonly container: HTMLElement;
  private readonly callbacks: SceneCallbacks;

  /** Root group holding the authored ship mesh (placeholder until a GLB loads). */
  private shipRoot = new THREE.Group();
  private hullMesh: THREE.Object3D;

  private readonly markers: Marker[] = [];
  private readonly markerGroup = new THREE.Group();
  private selected: Marker | null = null;

  private colliderHelper: THREE.LineSegments | null = null;
  private colliderVisible = false;

  constructor(container: HTMLElement, callbacks: SceneCallbacks = {}) {
    this.container = container;
    this.callbacks = callbacks;

    const { clientWidth: w, clientHeight: h } = container;

    this.scene.background = new THREE.Color(0x0c0f14);
    this.scene.fog = new THREE.FogExp2(0x0c0f14, 0.012);

    this.camera = new THREE.PerspectiveCamera(50, w / h || 1, 0.1, 2000);
    this.camera.position.set(8, 6, 10);

    this.renderer = new THREE.WebGLRenderer({ antialias: true });
    this.renderer.setPixelRatio(window.devicePixelRatio);
    this.renderer.setSize(w, h);
    container.appendChild(this.renderer.domElement);

    // Lighting: soft ambient + a key directional light.
    this.scene.add(new THREE.AmbientLight(0xb8c4d8, 0.6));
    const key = new THREE.DirectionalLight(0xffffff, 2.2);
    key.position.set(5, 10, 7);
    this.scene.add(key);
    const fill = new THREE.DirectionalLight(0x88aaff, 0.5);
    fill.position.set(-6, -3, -5);
    this.scene.add(fill);

    // Helpers.
    const grid = new THREE.GridHelper(40, 40, 0x2c5d96, 0x1b2230);
    (grid.material as THREE.Material).depthWrite = false;
    this.scene.add(grid);
    this.scene.add(new THREE.AxesHelper(2));

    // Default placeholder ship.
    this.hullMesh = this.makePlaceholderMesh();
    this.shipRoot.add(this.hullMesh);
    this.scene.add(this.shipRoot);

    this.scene.add(this.markerGroup);

    // Controls.
    this.orbit = new OrbitControls(this.camera, this.renderer.domElement);
    this.orbit.enableDamping = true;
    this.orbit.dampingFactor = 0.08;

    this.transform = new TransformControls(this.camera, this.renderer.domElement);
    this.transform.addEventListener('dragging-changed', (e) => {
      this.orbit.enabled = !e.value;
    });
    this.transform.addEventListener('objectChange', () => {
      if (this.selected) {
        this.callbacks.onMarkerMoved?.(
          this.selected,
          this.selected.object.position.clone(),
        );
      }
    });
    // TransformControls is a Object3D-backed helper in recent three; add its helper.
    const gizmo = this.transform.getHelper();
    this.scene.add(gizmo);

    this.renderer.domElement.addEventListener('pointerdown', this.onPointerDown);
    window.addEventListener('resize', this.onResize);

    this.animate();
  }

  private makePlaceholderMesh(): THREE.Mesh {
    // A simple elongated hull so orientation is obvious. Named "hull" so the
    // exported node aligns with the default collider's `node` field.
    const geo = new THREE.ConeGeometry(1.1, 4, 6);
    geo.rotateX(Math.PI / 2);
    geo.computeVertexNormals();
    const mat = new THREE.MeshStandardMaterial({
      color: 0x9fb2c8,
      metalness: 0.5,
      roughness: 0.45,
      flatShading: true,
    });
    const mesh = new THREE.Mesh(geo, mat);
    mesh.name = 'hull';
    return mesh;
  }

  // ---- GLB import ---------------------------------------------------------

  async importGLB(file: File): Promise<void> {
    const buffer = await file.arrayBuffer();
    const loader = new GLTFLoader();
    const gltf = await loader.parseAsync(buffer, '');
    const root = gltf.scene;

    // Name the top hull node "hull" if nothing already claims it, so the
    // default convexHull collider resolves on the Rust side.
    if (!root.getObjectByName('hull')) {
      root.name = 'hull';
    }

    this.scene.remove(this.shipRoot);
    this.disposeObject(this.shipRoot);

    this.shipRoot = new THREE.Group();
    this.shipRoot.add(root);
    this.hullMesh = root;
    this.scene.add(this.shipRoot);

    this.frameObject(this.shipRoot);
    if (this.colliderVisible) this.refreshColliderHelper();
  }

  // ---- Markers (hardpoints / subsystems) ----------------------------------

  addMarker(kind: MarkerKind, refId: string, position?: THREE.Vector3): Marker {
    const color = kind === 'hardpoint' ? HARDPOINT_COLOR : SUBSYSTEM_COLOR;
    const group = new THREE.Group();

    const geo =
      kind === 'hardpoint'
        ? new THREE.OctahedronGeometry(0.28)
        : new THREE.BoxGeometry(0.4, 0.4, 0.4);
    const mat = new THREE.MeshBasicMaterial({ color, wireframe: true });
    group.add(new THREE.Mesh(geo, mat));

    if (position) group.position.copy(position);

    const marker: Marker = { kind, refId, object: group };
    group.userData.marker = marker;
    this.markerGroup.add(group);
    this.markers.push(marker);
    return marker;
  }

  removeMarker(kind: MarkerKind, refId: string): void {
    const idx = this.markers.findIndex((m) => m.kind === kind && m.refId === refId);
    if (idx === -1) return;
    const [marker] = this.markers.splice(idx, 1);
    if (this.selected === marker) this.select(null);
    this.markerGroup.remove(marker.object);
    this.disposeObject(marker.object);
  }

  /** Re-key a marker when its underlying entry id changes. */
  renameMarker(kind: MarkerKind, oldId: string, newId: string): void {
    const marker = this.markers.find((m) => m.kind === kind && m.refId === oldId);
    if (marker) marker.refId = newId;
  }

  getMarkerPosition(kind: MarkerKind, refId: string): THREE.Vector3 | null {
    const marker = this.findMarker(kind, refId);
    return marker ? marker.object.position.clone() : null;
  }

  findMarker(kind: MarkerKind, refId: string): Marker | null {
    return this.markers.find((m) => m.kind === kind && m.refId === refId) ?? null;
  }

  select(marker: Marker | null): void {
    this.selected = marker;
    if (marker) {
      this.transform.attach(marker.object);
    } else {
      this.transform.detach();
    }
    this.callbacks.onSelectionChanged?.(marker);
  }

  setTransformMode(mode: 'translate' | 'rotate' | 'scale'): void {
    this.transform.setMode(mode);
  }

  // ---- Collider visualization (stub) --------------------------------------

  setColliderVisible(visible: boolean): void {
    this.colliderVisible = visible;
    if (visible) {
      this.refreshColliderHelper();
    } else if (this.colliderHelper) {
      this.scene.remove(this.colliderHelper);
      this.disposeObject(this.colliderHelper);
      this.colliderHelper = null;
    }
  }

  private refreshColliderHelper(): void {
    if (this.colliderHelper) {
      this.scene.remove(this.colliderHelper);
      this.disposeObject(this.colliderHelper);
      this.colliderHelper = null;
    }
    // Stub: wireframe of the mesh's world-space AABB. The real tool authors a
    // convex hull / primitive set; sol-shipc validates watertight/convex.
    const box = new THREE.Box3().setFromObject(this.shipRoot);
    if (box.isEmpty()) return;
    const size = new THREE.Vector3();
    const center = new THREE.Vector3();
    box.getSize(size);
    box.getCenter(center);
    const geo = new THREE.BoxGeometry(size.x, size.y, size.z);
    const edges = new THREE.EdgesGeometry(geo);
    const mat = new THREE.LineBasicMaterial({ color: 0x39d98a });
    this.colliderHelper = new THREE.LineSegments(edges, mat);
    this.colliderHelper.position.copy(center);
    this.scene.add(this.colliderHelper);
    geo.dispose();
  }

  // ---- Export -------------------------------------------------------------

  async exportGLB(): Promise<ArrayBuffer> {
    // Export only the ship geometry (not helpers/markers/gizmos).
    // TODO: emit hardpoint/subsystem anchors as named child nodes (glTF extras)
    //       so node names fully round-trip with the ShipDef.
    const exporter = new GLTFExporter();
    const result = await exporter.parseAsync(this.shipRoot, { binary: true });
    return result as ArrayBuffer;
  }

  // ---- Camera framing -----------------------------------------------------

  frameObject(object: THREE.Object3D): void {
    const box = new THREE.Box3().setFromObject(object);
    if (box.isEmpty()) return;
    const size = box.getSize(new THREE.Vector3());
    const center = box.getCenter(new THREE.Vector3());
    const radius = Math.max(size.x, size.y, size.z) * 0.5 || 1;
    const dist = radius / Math.sin((this.camera.fov * Math.PI) / 360);
    const dir = new THREE.Vector3(0.7, 0.5, 1).normalize();
    this.camera.position.copy(center).addScaledVector(dir, dist * 1.4);
    this.orbit.target.copy(center);
    this.orbit.update();
  }

  // ---- Internals ----------------------------------------------------------

  private onPointerDown = (event: PointerEvent): void => {
    if (this.transform.dragging) return;
    const rect = this.renderer.domElement.getBoundingClientRect();
    this.pointer.x = ((event.clientX - rect.left) / rect.width) * 2 - 1;
    this.pointer.y = -((event.clientY - rect.top) / rect.height) * 2 + 1;
    this.raycaster.setFromCamera(this.pointer, this.camera);

    const targets = this.markers.map((m) => m.object);
    const hits = this.raycaster.intersectObjects(targets, true);
    if (hits.length === 0) return;

    // Walk up to the marker group that carries userData.marker.
    let obj: THREE.Object3D | null = hits[0].object;
    while (obj && !obj.userData.marker) obj = obj.parent;
    if (obj && obj.userData.marker) {
      this.select(obj.userData.marker as Marker);
    }
  };

  private onResize = (): void => {
    const { clientWidth: w, clientHeight: h } = this.container;
    if (w === 0 || h === 0) return;
    this.camera.aspect = w / h;
    this.camera.updateProjectionMatrix();
    this.renderer.setSize(w, h);
  };

  private animate = (): void => {
    requestAnimationFrame(this.animate);
    this.orbit.update();
    this.renderer.render(this.scene, this.camera);
  };

  private disposeObject(root: THREE.Object3D): void {
    root.traverse((obj) => {
      const mesh = obj as Partial<THREE.Mesh> & THREE.Object3D;
      mesh.geometry?.dispose?.();
      const mat = (mesh as THREE.Mesh).material;
      if (Array.isArray(mat)) mat.forEach((m) => m.dispose());
      else mat?.dispose?.();
    });
  }
}
