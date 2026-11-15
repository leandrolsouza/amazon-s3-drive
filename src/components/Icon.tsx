const paths = {
 bucket: "M4 6c0 1.7 3.6 3 8 3s8-1.3 8-3-3.6-3-8-3-8 1.3-8 3Zm0 0 1.5 12c0 1.7 2.9 3 6.5 3s6.5-1.3 6.5-3L20 6",
 settings: "M4 7h16M4 17h16M9 4v6m6 4v6",
 terminal: "m5 6 5 5-5 5m8 2h6",
 open: "M14 3h7v7M21 3 10 14M10 3H5v18h16v-9",
 folder: "M3 7V3h7l2 3h9v15H3V7Z",
 link: "m10 13 4-4M8 16l-1 1a4 4 0 0 1-6-6l4-4a4 4 0 0 1 6 0m2 1 1-1a4 4 0 0 1 6 6l-4 4a4 4 0 0 1-6 0",
 info: "M12 11v6M12 7h.01",
 retry: "M20 7v5h-5M4 17v-5h5M5 7a8 8 0 0 1 13-2l2 3M4 16l2 3a8 8 0 0 0 13-2",
 chart: "M4 3v17h17M8 15v-4m5 4V7m5 8v-6",
 upload: "M12 16V3m-5 5 5-5 5 5M3 15v6h18v-6",
 list: "M8 6h13M8 12h13M8 18h13M3 6h.01M3 12h.01M3 18h.01",
 folderPlus: "M3 7V3h7l2 3h9v15H3V7ZM12 10v7m-3.5-3.5h7",
 play: "m8 4 12 8-12 8V4Z",
 stop: "M6 6h12v12H6z",
 chevronRight: "m9 6 6 6-6 6",
 close: "m6 6 12 12M6 18 18 6",
 minimize: "M5 12h14",
 maximize: "M6 6h12v12H6z",
 restore: "M6 9h9v9H6zM9 9V6h9v9h-3",
} as const;
export function Icon({ name }: { name: keyof typeof paths }) {
 return <svg width="17" height="17" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.7" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">{name === "info" && <circle cx="12" cy="12" r="9" />}<path d={paths[name]} /></svg>;
}
