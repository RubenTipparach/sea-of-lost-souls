import type { ShipDef, Hardpoint, Subsystem } from './shipDef';
import {
  el,
  section,
  textField,
  numberField,
  selectField,
} from './dom';

export interface PanelHandlers {
  onChanged: () => void;
  onSelectHardpoint: (id: string) => void;
  onAddHardpoint: () => void;
  onRemoveHardpoint: (id: string) => void;
  onRenameHardpoint: (oldId: string, newId: string) => void;
  onSelectSubsystem: (id: string) => void;
  onAddSubsystem: () => void;
  onRemoveSubsystem: (id: string) => void;
  onRenameSubsystem: (oldId: string, newId: string) => void;
}

const FACTIONS = ['player', 'vaered', 'corsair', 'neutral'];
const CLASSES = ['strike', 'frigate', 'cruiser', 'capital', 'utility'];

/** Renders and re-renders the ShipDef editing panel into a host element. */
export class Panel {
  private readonly host: HTMLElement;
  private readonly def: ShipDef;
  private readonly handlers: PanelHandlers;
  private selectedHardpoint: string | null = null;
  private selectedSubsystem: string | null = null;

  constructor(host: HTMLElement, def: ShipDef, handlers: PanelHandlers) {
    this.host = host;
    this.def = def;
    this.handlers = handlers;
  }

  setSelectedHardpoint(id: string | null): void {
    this.selectedHardpoint = id;
    if (id) this.selectedSubsystem = null;
    this.render();
  }

  setSelectedSubsystem(id: string | null): void {
    this.selectedSubsystem = id;
    if (id) this.selectedHardpoint = null;
    this.render();
  }

  render(): void {
    this.host.replaceChildren(
      el('h1', {}, ['Sea of Lost Souls']),
      el('p', { class: 'subtitle' }, ['Ship Editor — authors model.glb + ship.json']),
      this.identitySection(),
      this.crewHullSection(),
      this.shieldsSection(),
      this.powerSection(),
      this.hardpointsSection(),
    );
  }

  private touch(): void {
    this.handlers.onChanged();
  }

  private identitySection(): HTMLElement {
    const d = this.def;
    return section(
      'Identity',
      el('div', { class: 'section-body' }, [
        textField('id', d.id, (v) => {
          d.id = v;
          this.touch();
        }),
        textField('name', d.name, (v) => {
          d.name = v;
          this.touch();
        }),
        selectField('faction', d.faction, FACTIONS, (v) => {
          d.faction = v;
          this.touch();
        }),
        selectField('class', d.class, CLASSES, (v) => {
          d.class = v;
          this.touch();
        }),
        numberField('mass', d.mass, (v) => {
          d.mass = v;
          this.touch();
        }),
      ]),
    );
  }

  private crewHullSection(): HTMLElement {
    const d = this.def;
    return section(
      'Crew & Hull',
      el('div', { class: 'section-body' }, [
        numberField('crew.min', d.crew.min, (v) => {
          d.crew.min = v;
          this.touch();
        }),
        numberField('crew.optimal', d.crew.optimal, (v) => {
          d.crew.optimal = v;
          this.touch();
        }),
        numberField('hull.integrity', d.hull.integrity, (v) => {
          d.hull.integrity = v;
          this.touch();
        }),
        textField('hull.sections', d.hull.sections.join(', '), (v) => {
          d.hull.sections = v
            .split(',')
            .map((s) => s.trim())
            .filter(Boolean);
          this.touch();
        }),
      ]),
    );
  }

  private shieldsSection(): HTMLElement {
    const s = this.def.shields;
    return section(
      'Shields',
      el('div', { class: 'section-body' }, [
        numberField('facings', s.facings, (v) => {
          s.facings = v;
          this.touch();
        }),
        numberField('capacity', s.capacity, (v) => {
          s.capacity = v;
          this.touch();
        }),
        numberField('regen', s.regen, (v) => {
          s.regen = v;
          this.touch();
        }),
      ]),
    );
  }

  private powerSection(): HTMLElement {
    const g = this.def.powerGrid;
    const body = el('div', { class: 'section-body' }, [
      numberField('reactor.output', g.reactor.output, (v) => {
        g.reactor.output = v;
        this.touch();
      }),
      textField('reactor.node', g.reactor.node, (v) => {
        g.reactor.node = v;
        this.touch();
      }),
    ]);
    body.append(el('p', { class: 'hint' }, ['Subsystems:']));
    for (const sub of g.subsystems) {
      body.append(this.subsystemItem(sub));
    }
    const addBtn = el('button', { class: 'tiny' }, ['+ Subsystem']);
    addBtn.addEventListener('click', () => this.handlers.onAddSubsystem());
    body.append(el('div', { class: 'row' }, [addBtn]));
    return section('Power Grid', body);
  }

  private subsystemItem(sub: Subsystem): HTMLElement {
    const selected = this.selectedSubsystem === sub.id;
    const remove = el('button', { class: 'tiny danger' }, ['×']);
    remove.addEventListener('click', (e) => {
      e.stopPropagation();
      this.handlers.onRemoveSubsystem(sub.id);
    });
    const head = el('div', { class: 'list-item-head' }, [
      el('span', { class: 'title' }, [sub.id || '(unnamed)']),
      remove,
    ]);
    const item = el('div', { class: selected ? 'list-item selected' : 'list-item' }, [
      head,
      textField('id', sub.id, (v) => {
        const old = sub.id;
        sub.id = v;
        this.handlers.onRenameSubsystem(old, v);
        this.touch();
      }),
      textField('node', sub.node, (v) => {
        sub.node = v;
        this.touch();
      }),
      numberField('draw', sub.draw, (v) => {
        sub.draw = v;
        this.touch();
      }),
      numberField('priority', sub.priority, (v) => {
        sub.priority = v;
        this.touch();
      }),
    ]);
    item.addEventListener('click', () => this.handlers.onSelectSubsystem(sub.id));
    return item;
  }

  private hardpointsSection(): HTMLElement {
    const body = el('div', { class: 'section-body' }, []);
    for (const hp of this.def.hardpoints) {
      body.append(this.hardpointItem(hp));
    }
    const addBtn = el('button', { class: 'tiny' }, ['+ Hardpoint']);
    addBtn.addEventListener('click', () => this.handlers.onAddHardpoint());
    body.append(el('div', { class: 'row' }, [addBtn]));
    return section('Hardpoints', body);
  }

  private hardpointItem(hp: Hardpoint): HTMLElement {
    const selected = this.selectedHardpoint === hp.id;
    const remove = el('button', { class: 'tiny danger' }, ['×']);
    remove.addEventListener('click', (e) => {
      e.stopPropagation();
      this.handlers.onRemoveHardpoint(hp.id);
    });
    const head = el('div', { class: 'list-item-head' }, [
      el('span', { class: 'title' }, [hp.id || '(unnamed)']),
      remove,
    ]);
    const item = el('div', { class: selected ? 'list-item selected' : 'list-item' }, [
      head,
      textField('id', hp.id, (v) => {
        const old = hp.id;
        hp.id = v;
        this.handlers.onRenameHardpoint(old, v);
        this.touch();
      }),
      textField('node', hp.node, (v) => {
        hp.node = v;
        this.touch();
      }),
      selectField('mount', hp.mount, ['fixed', 'turret'], (v) => {
        hp.mount = v as Hardpoint['mount'];
        this.touch();
      }),
      textField('weapon', hp.weapon, (v) => {
        hp.weapon = v;
        this.touch();
      }),
      numberField('arcDeg', hp.arcDeg, (v) => {
        hp.arcDeg = v;
        this.touch();
      }),
      numberField('drawPerShot', hp.drawPerShot, (v) => {
        hp.drawPerShot = v;
        this.touch();
      }),
    ]);
    item.addEventListener('click', () => this.handlers.onSelectHardpoint(hp.id));
    return item;
  }
}
