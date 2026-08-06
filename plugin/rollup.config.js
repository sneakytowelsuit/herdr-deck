import commonjs from "@rollup/plugin-commonjs";
import nodeResolve from "@rollup/plugin-node-resolve";
import typescript from "@rollup/plugin-typescript";

/**
 * Bundle the plugin into the single file the Stream Deck app runs.
 *
 * The app launches `CodePath` with its own bundled Node, so everything the plugin needs must be
 * in that one file — there is no `node_modules` beside it at runtime.
 */
export default {
	input: "src/plugin.ts",
	output: {
		file: "com.sneakytowelsuit.herdr-deck.sdPlugin/bin/plugin.js",
		format: "es",
		sourcemap: true,
		sourcemapPathTransform: (relative) => relative.replace(/^\.\.\//, ""),
	},
	// Node built-ins stay external: the runtime provides them.
	external: [/^node:/],
	plugins: [
		typescript({ tsconfig: "./tsconfig.json", outDir: undefined, declaration: false }),
		nodeResolve({ browser: false, exportConditions: ["node"], preferBuiltins: true }),
		commonjs(),
	],
};
