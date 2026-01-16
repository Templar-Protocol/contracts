#!/usr/bin/env npx tsx
import * as fs from "fs"
import * as path from "path"
import { fileURLToPath } from "url"

const __filename = fileURLToPath(import.meta.url)
const __dirname = path.dirname(__filename)

const ABI_PATH = path.resolve(__dirname, "../src/abi/generated/vault.abi.json")
const OUTPUT_PATH = path.resolve(__dirname, "../src/abi/generated/types.ts")

type JsonSchema = {
  type?: string | string[]
  $ref?: string
  description?: string
  properties?: Record<string, JsonSchema>
  required?: string[]
  additionalProperties?: boolean | JsonSchema
  items?: JsonSchema | JsonSchema[]
  anyOf?: JsonSchema[]
  oneOf?: JsonSchema[]
  allOf?: JsonSchema[]
  enum?: (string | number | boolean | null)[]
  const?: unknown
  format?: string
  minimum?: number
  maximum?: number
  pattern?: string
  minItems?: number
  maxItems?: number
  uniqueItems?: boolean
}

type AbiRoot = {
  schema_version: string
  body: {
    functions: Array<{
      name: string
      kind: string
      params?: { args?: Array<{ name: string; type_schema: JsonSchema }> }
      result?: { type_schema: JsonSchema }
    }>
    root_schema: {
      definitions: Record<string, JsonSchema>
    }
  }
}

function refToTypeName(ref: string): string {
  const match = ref.match(/#\/definitions\/(.+)$/)
  return match ? match[1] : ref
}

function schemaToTs(schema: JsonSchema, definitions: Record<string, JsonSchema>, indent = 0): string {
  const pad = "  ".repeat(indent)

  if (schema.$ref) {
    const typeName = refToTypeName(schema.$ref)
    return sanitizeTypeName(typeName)
  }

  if (schema.anyOf) {
    const types = schema.anyOf.map((s) => schemaToTs(s, definitions, indent))
    if (types.includes("null")) {
      const nonNull = types.filter((t) => t !== "null")
      return nonNull.length === 1 ? `${nonNull[0]} | null` : `(${nonNull.join(" | ")}) | null`
    }
    return types.join(" | ")
  }

  if (schema.oneOf) {
    return schema.oneOf.map((s) => schemaToTs(s, definitions, indent)).join("\n  | ")
  }

  if (schema.allOf) {
    const types = schema.allOf.map((s) => schemaToTs(s, definitions, indent))
    return types.join(" & ")
  }

  if (schema.enum) {
    return schema.enum.map((v) => (typeof v === "string" ? `"${v}"` : String(v))).join(" | ")
  }

  if (schema.const !== undefined) {
    return typeof schema.const === "string" ? `"${schema.const}"` : String(schema.const)
  }

  if (schema.type === "null") {
    return "null"
  }

  if (schema.type === "string") {
    return "string"
  }

  if (schema.type === "integer" || schema.type === "number") {
    return "number"
  }

  if (schema.type === "boolean") {
    return "boolean"
  }

  if (schema.type === "array") {
    if (schema.items) {
      if (Array.isArray(schema.items)) {
        const tupleTypes = schema.items.map((item) => schemaToTs(item, definitions, indent))
        return `readonly [${tupleTypes.join(", ")}]`
      }
      const itemType = schemaToTs(schema.items, definitions, indent)
      if (itemType.startsWith("readonly ")) {
        return `${itemType}[]`
      }
      return `readonly ${itemType}[]`
    }
    return "readonly unknown[]"
  }

  if (schema.type === "object" || schema.properties) {
    if (!schema.properties || Object.keys(schema.properties).length === 0) {
      return "Record<string, unknown>"
    }

    const required = new Set(schema.required || [])
    const props = Object.entries(schema.properties).map(([key, propSchema]) => {
      const opt = required.has(key) ? "" : "?"
      const propType = schemaToTs(propSchema, definitions, indent + 1)
      return `${pad}  readonly ${key}${opt}: ${propType}`
    })

    return `{\n${props.join("\n")}\n${pad}}`
  }

  if (Array.isArray(schema.type)) {
    const types = schema.type.map((t) => {
      if (t === "null") return "null"
      if (t === "string") return "string"
      if (t === "integer" || t === "number") return "number"
      if (t === "boolean") return "boolean"
      if (t === "array") return "unknown[]"
      if (t === "object") return "Record<string, unknown>"
      return "unknown"
    })
    return types.join(" | ")
  }

  return "unknown"
}

const TYPE_ALIASES: Record<string, string> = {
  "Accumulator_for_BorrowAsset": "Accumulator",
  "FungibleAsset_for_BorrowAsset": "FungibleAsset",
  "FungibleAssetAmount_for_BorrowAsset": "FungibleAssetAmount",
  "Fee_for_String": "Fee",
  "Fee_for_Wad": "FeeWad",
  "Fees_for_String": "Fees",
  "Fees_for_Wad": "FeesWad",
  "PendingValue_for_TimelockedAction": "PendingTimelockedAction",
}

function sanitizeTypeName(name: string): string {
  if (TYPE_ALIASES[name]) {
    return TYPE_ALIASES[name]
  }
  return name
    .replace(/_for_/g, "_")
    .replace(/_of_/g, "_")
    .replace(/</g, "_")
    .replace(/>/g, "")
    .replace(/,\s*/g, "_")
    .replace(/\s+/g, "")
}

function shouldSkipDefinition(name: string): boolean {
  const skip = [
    "Promise",
    "PromiseOrValue",
  ]
  return skip.some((s) => name.startsWith(s))
}

function generateTypes(abi: AbiRoot): string {
  const definitions = abi.body.root_schema.definitions
  const lines: string[] = []

  lines.push("// Auto-generated from vault.abi.json - DO NOT EDIT")
  lines.push(`// ABI schema version: ${abi.schema_version}`)
  lines.push(`// Generated: ${new Date().toISOString()}`)
  lines.push("")

  const sortedDefs = Object.entries(definitions).sort(([a], [b]) => a.localeCompare(b))

  for (const [name, schema] of sortedDefs) {
    if (shouldSkipDefinition(name)) continue

    const typeName = sanitizeTypeName(name)
    const tsType = schemaToTs(schema, definitions)

    if (schema.description) {
      const desc = schema.description.split("\n")[0].slice(0, 80)
      lines.push(`/** ${desc} */`)
    }

    if (schema.oneOf || schema.enum) {
      lines.push(`export type ${typeName} =`)
      lines.push(`  | ${tsType}`)
    } else {
      lines.push(`export type ${typeName} = ${tsType}`)
    }
    lines.push("")
  }

  lines.push("// Primitive type aliases")
  lines.push("export type AccountId = string")
  lines.push("export type U128 = string")
  lines.push("export type U64 = string")
  lines.push("")

  lines.push("// Method argument types")
  lines.push("export type StorageDepositArgs = {")
  lines.push("  readonly account_id?: AccountId | null")
  lines.push("  readonly registration_only?: boolean | null")
  lines.push("}")
  lines.push("")
  lines.push("export type WithdrawArgs = {")
  lines.push("  readonly amount: U128")
  lines.push("  readonly receiver: AccountId")
  lines.push("}")
  lines.push("")
  lines.push("export type RedeemArgs = {")
  lines.push("  readonly shares: U128")
  lines.push("  readonly receiver: AccountId")
  lines.push("}")
  lines.push("")
  lines.push("export type RefreshMarketsArgs = {")
  lines.push("  readonly markets: readonly MarketId[]")
  lines.push("}")
  lines.push("")
  lines.push("export type FtTransferCallArgs = {")
  lines.push("  readonly receiver_id: AccountId")
  lines.push("  readonly amount: U128")
  lines.push("  readonly memo?: string | null")
  lines.push("  readonly msg: string")
  lines.push("}")
  lines.push("")
  lines.push("export type DepositMsg = \"Supply\"")
  lines.push("export const DEPOSIT_MSG_SUPPLY: DepositMsg = \"Supply\"")
  lines.push("")

  return lines.join("\n")
}

function main() {
  console.log(`Reading ABI from: ${ABI_PATH}`)
  const abiContent = fs.readFileSync(ABI_PATH, "utf-8")
  const abi = JSON.parse(abiContent) as AbiRoot

  console.log(`Found ${Object.keys(abi.body.root_schema.definitions).length} definitions`)

  const output = generateTypes(abi)

  console.log(`Writing types to: ${OUTPUT_PATH}`)
  fs.writeFileSync(OUTPUT_PATH, output, "utf-8")

  console.log("Done!")
}

main()
