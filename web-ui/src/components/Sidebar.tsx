import { For, Show, createResource, createSignal } from "solid-js";
import { NavLink, NavSection } from "@forge/ui";
import { BookOpen, Boxes, ChevronDown, ChevronRight, Library, PackageOpen, Server, Workflow } from "lucide-solid";
import { listServices } from "../api";
import { servicesRevision, setView, view } from "../store";

export default function Sidebar() {
  const [services] = createResource(servicesRevision, listServices);
  const [libraryOpen, setLibraryOpen] = createSignal(true);
  return <>
    <NavSection>
      <NavLink icon={Server} active={view().kind === "services"} onClick={() => setView({ kind: "services" })}>Services</NavLink>
      <For each={services() ?? []}>{(service) => <NavLink active={(view().kind === "service" || view().kind === "sysrun") && (view() as any).name === service.name || view().kind === "sysrun" && (view() as any).service === service.name} onClick={() => setView({ kind: "service", name: service.name })} style={{ "padding-left": "24px" }}><Boxes size={14} />{service.name}</NavLink>}</For>
    </NavSection>
    <NavSection>
      <NavLink icon={Workflow} active={view().kind === "pipelines" || view().kind === "pipeline" || view().kind === "pipelinerun"} onClick={() => setView({ kind: "pipelines" })}>Pipelines</NavLink>
    </NavSection>
    <NavSection>
      <button class="sidebar-heading sidebar-toggle" aria-expanded={libraryOpen()} onClick={() => setLibraryOpen(!libraryOpen())}>{libraryOpen() ? <ChevronDown size={13} /> : <ChevronRight size={13} />}<Library size={14} /> Library</button>
      <Show when={libraryOpen()}>
        <NavLink icon={BookOpen} active={view().kind === "runbooks" || view().kind === "runbook"} onClick={() => setView({ kind: "runbooks" })}>Playbooks</NavLink>
        <NavLink icon={PackageOpen} active={view().kind === "packages" || view().kind === "package"} onClick={() => setView({ kind: "packages" })}>Packages</NavLink>
      </Show>
    </NavSection>
  </>;
}
