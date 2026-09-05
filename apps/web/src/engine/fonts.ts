/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at http://mozilla.org/MPL/2.0/. */

/**
 * The half of the font story that needs a browser the runtime cannot assume:
 * the machine's own families, behind the Local Font Access API. Web fonts and
 * loading live in `@diffusionstudio/runtime` (`getWebFonts`, `loadWebFont`),
 * which the CLI shares.
 */

import { FontStyle } from '@diffusionstudio/runtime';

import type { FontSources } from '@diffusionstudio/runtime';

interface LocalFont {
	family: string;
	fullName: string;
	postscriptName: string;
	style: string;
}

/**
 * Every family installed on the machine, grouped by family. Requires the Local
 * Font Access API and a granted permission; the caller handles the rejection.
 */
export async function getLocalFonts(): Promise<FontSources[]> {
	const families = new Map<string, LocalFont[]>();

	for (const font of (await window.queryLocalFonts()) as LocalFont[]) {
		const variants = families.get(font.family);
		if (variants) variants.push(font);
		else families.set(font.family, [font]);
	}

	return [...families].map(([family, fonts]) => ({
		family,
		variants: fonts.map((font) => ({
			family,
			source: `local('${font.fullName}'), local('${font.postscriptName}')`,
			weight: matchFontWeight(font.style),
			style: matchFontStyle(font.style),
		})),
	}));
}

/** The style a local font names itself in, as the runtime spells it. */
function matchFontStyle(style: string): FontStyle {
	if (/oblique/i.test(style)) return FontStyle.OBLIQUE;
	if (/italic/i.test(style)) return FontStyle.ITALIC;
	return FontStyle.NORMAL;
}

/**
 * The CSS weight a local font's style name comes to. Ordered so the longer
 * names win: "extrabold" is not a bold, and "extralight" is not a light.
 */
function matchFontWeight(weight: string): string {
	const matches: [RegExp, string][] = [
		[/black|heavy/i, '900'],
		[/extrabold|ultrabold/i, '800'],
		[/semibold|demibold/i, '600'],
		[/bold|strong/i, '700'],
		[/medium/i, '500'],
		[/normal|regular|book|plain/i, '400'],
		[/extralight|ultralight/i, '200'],
		[/thin|hairline/i, '100'],
		[/light/i, '300'],
	];

	return matches.find(([pattern]) => pattern.test(weight))?.[1] ?? '400';
}
