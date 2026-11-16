// Import et export d'un agenda (.ics) ou d'un carnet (.vcf) (lot E8b) : un
// menu « Importer / exporter », avec la liste des agendas ou des carnets.
import { Button, Menu, MenuGroup, MenuGroupHeader, MenuItem, MenuList, MenuPopover, MenuTrigger } from "@fluentui/react-components";
import { ArrowDownload20Regular, ArrowExport20Regular, ArrowImport20Regular } from "@fluentui/react-icons";
import { api } from "../api";
import { t } from "../i18n";
import type { Account, PimCollection } from "../types";

interface Props {
  kind: "calendar" | "contacts";
  collections: PimCollection[];
  accounts: Account[];
  notify: (title: string, body?: string, intent?: "success" | "error" | "info") => void;
  /** Après un import : relire l'agenda ou les contacts. */
  onImported: () => void;
}

export function PimTransfer({ kind, collections, accounts, notify, onImported }: Props) {
  const calendar = kind === "calendar";
  const label = (c: PimCollection) =>
    c.account === "local" ? c.name : `${c.name} — ${accounts.find((a) => a.id === c.account)?.name ?? c.account}`;

  const exportTo = async (c: PimCollection) => {
    try {
      const { path, count } = await api.exportPim(c.url);
      notify(
        calendar
          ? count > 1
            ? t("{n} rendez-vous exportés", { n: count })
            : t("{n} rendez-vous exporté", { n: count })
          : count > 1
            ? t("{n} contacts exportés", { n: count })
            : t("{n} contact exporté", { n: count }),
        path,
      );
    } catch (e) {
      notify(t("Export impossible"), String(e), "error");
    }
  };

  const importInto = (c: PimCollection) => {
    const input = document.createElement("input");
    input.type = "file";
    input.multiple = true;
    input.accept = calendar ? ".ics,.ical,text/calendar" : ".vcf,.vcard,text/vcard,text/x-vcard";
    input.onchange = async () => {
      const files = Array.from(input.files ?? []);
      if (files.length === 0) return;
      let imported = 0;
      let failed = 0;
      let error = "";
      for (const file of files) {
        try {
          const report = await api.importPim(c.url, await file.text());
          imported += report.imported;
          failed += report.failed;
          if (report.error) error = report.error;
        } catch (e) {
          failed += 1;
          error = `${file.name} : ${String(e)}`;
        }
      }
      if (failed > 0) {
        notify(t("Import incomplet : {ok} importés, {failed} refusés", { ok: imported, failed }), error, "error");
      } else {
        notify(
          calendar
            ? imported > 1
              ? t("{n} rendez-vous importés", { n: imported })
              : t("{n} rendez-vous importé", { n: imported })
            : imported > 1
              ? t("{n} contacts importés", { n: imported })
              : t("{n} contact importé", { n: imported }),
          label(c),
        );
      }
      onImported();
    };
    input.click();
  };

  const list = (onPick: (c: PimCollection) => void) => (
    <MenuList>
      <MenuGroup>
        <MenuGroupHeader>{calendar ? t("Agendas") : t("Carnets d'adresses")}</MenuGroupHeader>
        {collections.map((c) => (
          <MenuItem key={c.url} onClick={() => onPick(c)}>
            {label(c)}
          </MenuItem>
        ))}
      </MenuGroup>
    </MenuList>
  );

  return (
    <Menu>
      <MenuTrigger disableButtonEnhancement>
        <Button appearance="subtle" icon={<ArrowDownload20Regular />} disabled={collections.length === 0}>
          {t("Importer / exporter")}
        </Button>
      </MenuTrigger>
      <MenuPopover>
        <MenuList>
          <Menu>
            <MenuTrigger disableButtonEnhancement>
              <MenuItem icon={<ArrowImport20Regular />}>{calendar ? t("Importer un fichier .ics dans") : t("Importer un fichier .vcf dans")}</MenuItem>
            </MenuTrigger>
            <MenuPopover>{list(importInto)}</MenuPopover>
          </Menu>
          <Menu>
            <MenuTrigger disableButtonEnhancement>
              <MenuItem icon={<ArrowExport20Regular />}>{calendar ? t("Exporter en .ics") : t("Exporter en .vcf")}</MenuItem>
            </MenuTrigger>
            <MenuPopover>{list(exportTo)}</MenuPopover>
          </Menu>
        </MenuList>
      </MenuPopover>
    </Menu>
  );
}
