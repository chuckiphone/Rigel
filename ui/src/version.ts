import packageJson from "../package.json";

/** The renderer version comes from the package, so About cannot drift from the build. */
export const APP_VERSION = packageJson.version;
