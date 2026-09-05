/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at http://mozilla.org/MPL/2.0/. */

export const WebFonts = {
	Inter: {
		weights: ['100', '200', '300', '400', '500', '600', '700', '800', '900'],
		url: 'https://fonts.gstatic.com/s/inter/v20/UcCo3FwrK3iLTcviYwYZ8UA3.woff2',
	},
	'The Bold Font': {
		weights: ['500'],
		url: 'https://diffusion-studio-public.s3.eu-central-1.amazonaws.com/fonts/the-bold-font.ttf',
	},
	'Komika Axis': {
		weights: ['400'],
		url: 'https://diffusion-studio-public.s3.eu-central-1.amazonaws.com/fonts/komika-axis.ttf',
	},
	Geologica: {
		weights: ['100', '200', '300', '400', '500', '600', '700', '800', '900'],
		url: 'https://fonts.gstatic.com/s/geologica/v1/oY1l8evIr7j9P3TN9YwNAdyjzUyDKkKdAGOJh1UlCDUIhAIdhCZOn1fLsig7jfvCCPHZckUWE1lELWNN-w.woff2',
	},
	Nunito: {
		weights: ['200', '300', '400', '500', '600', '700', '800', '900'],
		url: 'https://fonts.gstatic.com/s/nunito/v26/XRXV3I6Li01BKofINeaBTMnFcQ.woff2',
	},
	Figtree: {
		weights: ['300', '400', '500', '600', '700', '800', '900'],
		url: 'https://fonts.gstatic.com/s/figtree/v5/_Xms-HUzqDCFdgfMm4S9DaRvzig.woff2',
	},
	Urbanist: {
		weights: ['100', '200', '300', '400', '500', '600', '700', '800', '900'],
		url: 'https://fonts.gstatic.com/s/urbanist/v15/L0x-DF02iFML4hGCyMqlbS1miXK2.woff2',
	},
	Montserrat: {
		weights: ['100', '200', '300', '400', '500', '600', '700', '800', '900'],
		url: 'https://fonts.gstatic.com/s/montserrat/v26/JTUSjIg1_i6t8kCHKm459WlhyyTh89Y.woff2',
	},
	Bangers: {
		weights: ['400'],
		url: 'https://fonts.gstatic.com/s/bangers/v20/FeVQS0BTqb0h60ACH55Q2J5hm24.woff2',
	},
	Chewy: {
		weights: ['400'],
		url: 'https://fonts.gstatic.com/s/chewy/v18/uK_94ruUb-k-wn52KjI9OPec.woff2',
	},
	'Source Code Pro': {
		weights: ['200', '300', '400', '500', '600', '700', '800', '900'],
		url: 'https://fonts.gstatic.com/s/sourcecodepro/v22/HI_SiYsKILxRpg3hIP6sJ7fM7PqlPevWnsUnxg.woff2',
	},
} as const;

export const FONT_WEIGHTS = {
	'100': 'Thin',
	'200': 'Extra Light',
	'300': 'Light',
	'400': 'Normal',
	'500': 'Medium',
	'600': 'Semi Bold',
	'700': 'Bold',
	'800': 'Extra Bold',
	'900': 'Black',
} as const;
