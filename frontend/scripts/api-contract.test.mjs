import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import test from 'node:test';
import { fileURLToPath } from 'node:url';
import ts from 'typescript';
import { parseDocument } from 'yaml';

const frontendRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const repoRoot = path.resolve(frontendRoot, '..');
const clientPath = path.join(frontendRoot, 'src/api/client.ts');
const typesPath = path.join(frontendRoot, 'src/api/types.ts');
const openApiPath = path.join(repoRoot, 'docs/openapi.yaml');
const rustRouterPath = path.join(repoRoot, 'backend/crates/api/src/lib.rs');

const HTTP_METHODS = new Set(['get', 'post', 'put', 'patch', 'delete']);

// This is the deliberately frozen HTTP surface the future HttpApiClient and
// axum handlers will implement. Changing a path, verb, or success status is a
// reviewed contract change rather than a side effect of editing YAML.
const EXPECTED_OPERATIONS = {
  getMe: { method: 'GET', path: '/me', success: 200 },
  listTrips: { method: 'GET', path: '/trips', success: 200 },
  createTrip: { method: 'POST', path: '/trips', success: 201 },
  getTrip: { method: 'GET', path: '/trips/{tripId}', success: 200 },
  setTripStatus: { method: 'PATCH', path: '/trips/{tripId}/status', success: 200 },
  getUsers: { method: 'GET', path: '/trips/{tripId}/members', success: 200 },
  removeMember: { method: 'DELETE', path: '/trips/{tripId}/members/{userId}', success: 204 },
  invite: { method: 'POST', path: '/trips/{tripId}/invites', success: 201 },
  searchPlaces: { method: 'GET', path: '/trips/{tripId}/places/search', success: 200 },
  listCandidates: { method: 'GET', path: '/trips/{tripId}/candidates', success: 200 },
  addCandidate: { method: 'POST', path: '/trips/{tripId}/candidates', success: 201 },
  updateCandidate: { method: 'PATCH', path: '/trips/{tripId}/candidates/{candidateId}', success: 200 },
  setCandidateStatus: { method: 'PATCH', path: '/trips/{tripId}/candidates/{candidateId}/status', success: 200 },
  getCurrentPlan: { method: 'GET', path: '/trips/{tripId}/plan', success: 200 },
  initializePlan: { method: 'POST', path: '/trips/{tripId}/plan', success: 200 },
  listPlanVersions: { method: 'GET', path: '/trips/{tripId}/plan/versions', success: 200 },
  updateStop: { method: 'PATCH', path: '/trips/{tripId}/stops/{stopId}', success: 200 },
  updateDay: { method: 'PATCH', path: '/trips/{tripId}/days/{dayId}', success: 200 },
  updateNotice: { method: 'PATCH', path: '/trips/{tripId}/notices/{noticeId}', success: 200 },
  getHistory: { method: 'GET', path: '/trips/{tripId}/history', success: 200 },
  revertEdit: { method: 'POST', path: '/trips/{tripId}/edits/{editId}/revert', success: 204 },
  listProposals: { method: 'GET', path: '/trips/{tripId}/proposals', success: 200 },
  createProposal: { method: 'POST', path: '/trips/{tripId}/proposals', success: 201 },
  approveProposal: { method: 'POST', path: '/trips/{tripId}/proposals/{proposalId}/approve', success: 200 },
  rejectProposal: { method: 'POST', path: '/trips/{tripId}/proposals/{proposalId}/reject', success: 200 },
  proposalToPoll: { method: 'POST', path: '/trips/{tripId}/proposals/{proposalId}/to-poll', success: 201 },
  listPolls: { method: 'GET', path: '/trips/{tripId}/polls', success: 200 },
  createPoll: { method: 'POST', path: '/trips/{tripId}/polls', success: 201 },
  openPoll: { method: 'POST', path: '/trips/{tripId}/polls/{pollId}/open', success: 200 },
  vote: { method: 'POST', path: '/trips/{tripId}/polls/{pollId}/votes', success: 200 },
  closePoll: { method: 'POST', path: '/trips/{tripId}/polls/{pollId}/close', success: 200 },
  getReviewQueue: { method: 'GET', path: '/me/review-queue', success: 200 },
  approveReviewItem: { method: 'POST', path: '/me/review-queue/{itemId}/approve', success: 204 },
  rejectReviewItem: { method: 'POST', path: '/me/review-queue/{itemId}/reject', success: 204 },
  listThreads: { method: 'GET', path: '/trips/{tripId}/threads', success: 200 },
  createThread: { method: 'POST', path: '/trips/{tripId}/threads', success: 201 },
  getComments: { method: 'GET', path: '/trips/{tripId}/threads/{threadId}/comments', success: 200 },
  addComment: { method: 'POST', path: '/trips/{tripId}/threads/{threadId}/comments', success: 201 },
  toggleReaction: {
    method: 'POST',
    path: '/trips/{tripId}/threads/{threadId}/comments/{commentId}/reactions',
    success: 200,
  },
  getLedger: { method: 'GET', path: '/trips/{tripId}/ledger', success: 200 },
  addExpense: { method: 'POST', path: '/trips/{tripId}/expenses', success: 201 },
  updateExpense: { method: 'PATCH', path: '/trips/{tripId}/expenses/{expenseId}', success: 200 },
  deleteExpense: { method: 'DELETE', path: '/trips/{tripId}/expenses/{expenseId}', success: 204 },
  addSettlement: { method: 'POST', path: '/trips/{tripId}/settlements', success: 201 },
  listNotices: { method: 'GET', path: '/trips/{tripId}/notices', success: 200 },
  createNotice: { method: 'POST', path: '/trips/{tripId}/notices', success: 201 },
  toggleChecklistItem: {
    method: 'POST',
    path: '/trips/{tripId}/notices/{noticeId}/checklist/{itemId}/toggle',
    success: 200,
  },
  listTokens: { method: 'GET', path: '/me/tokens', success: 200 },
  createToken: { method: 'POST', path: '/me/tokens', success: 201 },
  revokeToken: { method: 'DELETE', path: '/me/tokens/{tokenId}', success: 204 },
};

const QUERY_PARAMETER_NAMES = {
  searchPlaces: { query: 'q' },
};

function sourceFile(file) {
  return ts.createSourceFile(file, fs.readFileSync(file, 'utf8'), ts.ScriptTarget.Latest, true);
}

function isExported(declaration) {
  return declaration.modifiers?.some((modifier) => modifier.kind === ts.SyntaxKind.ExportKeyword) ?? false;
}

function declarationName(node, source) {
  if (!node.name) throw new Error(`Unnamed declaration in ${source.fileName}`);
  return node.name.getText(source).replace(/^['"]|['"]$/g, '');
}

function parameterName(parameter, source) {
  if (!ts.isIdentifier(parameter.name)) throw new Error(`Unsupported parameter in ${source.fileName}`);
  return parameter.name.text;
}

function sorted(values) {
  return [...values].sort((left, right) => left.localeCompare(right));
}

function parseOpenApi() {
  const document = parseDocument(fs.readFileSync(openApiPath, 'utf8'), { uniqueKeys: true });
  assert.deepEqual(
    document.errors.map((error) => error.message),
    [],
    'docs/openapi.yaml must be valid YAML without duplicate keys',
  );
  return document.toJS();
}

function collectOperations(openapi) {
  const operations = new Map();
  for (const [route, pathItem] of Object.entries(openapi.paths ?? {})) {
    for (const [method, operation] of Object.entries(pathItem)) {
      if (!HTTP_METHODS.has(method)) continue;
      assert.equal(typeof operation.operationId, 'string', `${method.toUpperCase()} ${route} needs an operationId`);
      assert.ok(!operations.has(operation.operationId), `duplicate operationId: ${operation.operationId}`);
      operations.set(operation.operationId, { method: method.toUpperCase(), route, operation, pathItem });
    }
  }
  return operations;
}

function collectTypeScriptContract() {
  const sources = [sourceFile(typesPath), sourceFile(clientPath)];
  const declarations = new Map();
  let apiClient;

  for (const source of sources) {
    for (const statement of source.statements) {
      if (!isExported(statement) || (!ts.isInterfaceDeclaration(statement) && !ts.isTypeAliasDeclaration(statement))) {
        continue;
      }
      const name = declarationName(statement, source);
      assert.ok(!declarations.has(name), `duplicate exported contract declaration: ${name}`);
      declarations.set(name, { declaration: statement, source });
      if (name === 'ApiClient') apiClient = { declaration: statement, source };
    }
  }

  assert.ok(apiClient, 'client.ts must export ApiClient');
  const methods = new Map();
  for (const member of apiClient.declaration.members) {
    if (!ts.isMethodSignature(member)) continue;
    methods.set(declarationName(member, apiClient.source), member);
  }
  return { declarations, methods, apiSource: apiClient.source };
}

function localRefName(schema) {
  const ref = schema?.$ref;
  if (typeof ref !== 'string' || !ref.startsWith('#/components/schemas/')) return undefined;
  return ref.slice('#/components/schemas/'.length);
}

function typeDescriptor(typeNode, source) {
  if (typeNode.kind === ts.SyntaxKind.VoidKeyword) return { kind: 'void' };
  if (typeNode.kind === ts.SyntaxKind.StringKeyword) return { kind: 'string' };
  if (typeNode.kind === ts.SyntaxKind.NumberKeyword) return { kind: 'number' };
  if (typeNode.kind === ts.SyntaxKind.BooleanKeyword) return { kind: 'boolean' };
  if (ts.isArrayTypeNode(typeNode)) return { kind: 'array', items: typeDescriptor(typeNode.elementType, source) };
  if (ts.isTypeReferenceNode(typeNode)) return { kind: 'ref', name: typeNode.typeName.getText(source) };
  throw new Error(`Unsupported ApiClient type: ${typeNode.getText(source)}`);
}

function schemaDescriptor(schema) {
  const ref = localRefName(schema);
  if (ref) return { kind: 'ref', name: ref };
  if (schema?.type === 'array') return { kind: 'array', items: schemaDescriptor(schema.items) };
  if (schema?.type === 'integer' || schema?.type === 'number') return { kind: 'number' };
  if (schema?.type === 'string' || schema?.type === 'boolean') return { kind: schema.type };
  throw new Error(`Unsupported operation schema: ${JSON.stringify(schema)}`);
}

function promiseResult(method, source) {
  assert.ok(
    method.type && ts.isTypeReferenceNode(method.type),
    `${declarationName(method, source)} must return Promise<T>`,
  );
  assert.equal(method.type.typeName.getText(source), 'Promise');
  assert.equal(method.type.typeArguments?.length, 1);
  return typeDescriptor(method.type.typeArguments[0], source);
}

function resolveLocalRef(openapi, value) {
  if (!value?.$ref) return value;
  assert.match(value.$ref, /^#\//, `external references are not allowed in the frozen contract: ${value.$ref}`);
  return value.$ref
    .slice(2)
    .split('/')
    .map((part) => part.replaceAll('~1', '/').replaceAll('~0', '~'))
    .reduce((current, part) => current?.[part], openapi);
}

function operationParameters(openapi, record) {
  return [...(record.pathItem.parameters ?? []), ...(record.operation.parameters ?? [])].map((parameter) =>
    resolveLocalRef(openapi, parameter),
  );
}

function ownSchemaShape(schema) {
  const fragments = [schema, ...(schema.allOf ?? [])].filter((fragment) => fragment && !fragment.$ref);
  const properties = Object.assign({}, ...fragments.map((fragment) => fragment.properties ?? {}));
  const required = new Set(fragments.flatMap((fragment) => fragment.required ?? []));
  return { properties, required };
}

function ownInterfaceShape(declaration, source) {
  const properties = new Map();
  for (const member of declaration.members) {
    if (!ts.isPropertySignature(member)) continue;
    properties.set(declarationName(member, source), !member.questionToken);
  }
  return properties;
}

function stringEnumValues(type) {
  if (!ts.isUnionTypeNode(type)) return undefined;
  const values = [];
  for (const member of type.types) {
    if (!ts.isLiteralTypeNode(member) || !ts.isStringLiteral(member.literal)) return undefined;
    values.push(member.literal.text);
  }
  return values;
}

function discriminatedTypeScriptVariants(type) {
  if (!ts.isUnionTypeNode(type) || !type.types.every(ts.isTypeLiteralNode)) return undefined;
  return type.types.map((variant) => {
    const properties = new Map();
    let key;
    for (const member of variant.members) {
      if (!ts.isPropertySignature(member)) continue;
      const name = member.name.getText().replace(/^['"]|['"]$/g, '');
      properties.set(name, !member.questionToken);
      if (member.type && ts.isLiteralTypeNode(member.type) && ts.isStringLiteral(member.type.literal)) {
        key = member.type.literal.text;
      }
    }
    assert.ok(key, 'object-union variants need a string-literal discriminator');
    return { key, properties };
  });
}

function discriminatedOpenApiVariants(schema) {
  if (!Array.isArray(schema.oneOf)) return undefined;
  return schema.oneOf.map((variant) => {
    const discriminator = Object.entries(variant.properties ?? {}).find(([, property]) =>
      Object.hasOwn(property, 'const'),
    );
    assert.ok(discriminator, 'oneOf variants need a const discriminator');
    const required = new Set(variant.required ?? []);
    return {
      key: discriminator[1].const,
      properties: new Map(Object.keys(variant.properties ?? {}).map((name) => [name, required.has(name)])),
    };
  });
}

function assertPropertyShape(name, typeScriptProperties, openApiProperties) {
  assert.deepEqual(
    sorted(typeScriptProperties.keys()),
    sorted(openApiProperties.keys()),
    `${name} property names differ between TypeScript and OpenAPI`,
  );
  for (const [property, required] of typeScriptProperties) {
    assert.equal(
      openApiProperties.get(property),
      required,
      `${name}.${property} optionality differs between TypeScript and OpenAPI`,
    );
  }
}

test('ApiClient, OpenAPI operation IDs, and the intended route table are a bijection', () => {
  const openapi = parseOpenApi();
  const operations = collectOperations(openapi);
  const { methods } = collectTypeScriptContract();

  assert.deepEqual(sorted(methods.keys()), sorted(Object.keys(EXPECTED_OPERATIONS)));
  assert.deepEqual(sorted(operations.keys()), sorted(Object.keys(EXPECTED_OPERATIONS)));

  for (const [operationId, expected] of Object.entries(EXPECTED_OPERATIONS)) {
    const actual = operations.get(operationId);
    assert.deepEqual(
      { method: actual.method, path: actual.route },
      { method: expected.method, path: expected.path },
      `${operationId} route drifted`,
    );
    const successfulStatuses = Object.keys(actual.operation.responses ?? {}).filter((status) => /^2\d\d$/.test(status));
    assert.deepEqual(successfulStatuses, [String(expected.success)], `${operationId} success status drifted`);
  }
});

test('every operation request and success response matches its ApiClient signature', () => {
  const openapi = parseOpenApi();
  const operations = collectOperations(openapi);
  const { methods, apiSource } = collectTypeScriptContract();

  for (const [operationId, method] of methods) {
    const record = operations.get(operationId);
    const expected = EXPECTED_OPERATIONS[operationId];
    const parameters = operationParameters(openapi, record);
    const methodParameters = new Map(
      method.parameters.map((parameter) => [parameterName(parameter, apiSource), parameter]),
    );
    const pathNames = [...record.route.matchAll(/\{([^}]+)\}/g)].map((match) => match[1]);
    const declaredPathNames = parameters
      .filter((parameter) => parameter.in === 'path')
      .map((parameter) => parameter.name);
    assert.deepEqual(sorted(declaredPathNames), sorted(pathNames), `${operationId} path parameters drifted`);
    for (const name of pathNames) {
      const parameter = methodParameters.get(name);
      assert.ok(parameter, `${operationId} is missing ApiClient path parameter ${name}`);
      assert.deepEqual(typeDescriptor(parameter.type, apiSource), { kind: 'string' });
    }

    const queryMapping = QUERY_PARAMETER_NAMES[operationId] ?? {};
    const declaredQueryNames = parameters
      .filter((parameter) => parameter.in === 'query')
      .map((parameter) => parameter.name);
    assert.deepEqual(
      sorted(declaredQueryNames),
      sorted(Object.values(queryMapping)),
      `${operationId} query parameters drifted`,
    );
    for (const clientName of Object.keys(queryMapping)) {
      assert.ok(methodParameters.has(clientName), `${operationId} is missing ApiClient query parameter ${clientName}`);
    }

    const transportedNames = new Set([...pathNames, ...Object.keys(queryMapping)]);
    const bodyParameters = method.parameters.filter(
      (parameter) => !transportedNames.has(parameterName(parameter, apiSource)),
    );
    const bodySchema = record.operation.requestBody?.content?.['application/json']?.schema;

    if (bodyParameters.length === 0) {
      assert.equal(bodySchema, undefined, `${operationId} has an unexpected JSON body`);
    } else if (
      bodyParameters.length === 1 &&
      ['input', 'patch'].includes(parameterName(bodyParameters[0], apiSource))
    ) {
      assert.equal(record.operation.requestBody?.required, true, `${operationId} request body must be required`);
      assert.deepEqual(
        schemaDescriptor(bodySchema),
        typeDescriptor(bodyParameters[0].type, apiSource),
        `${operationId} request schema drifted`,
      );
    } else {
      assert.equal(record.operation.requestBody?.required, true, `${operationId} request body must be required`);
      assert.equal(bodySchema?.type, 'object', `${operationId} must wrap scalar arguments in a JSON object`);
      assert.deepEqual(
        sorted(Object.keys(bodySchema.properties ?? {})),
        sorted(bodyParameters.map((parameter) => parameterName(parameter, apiSource))),
        `${operationId} request fields drifted`,
      );
      assert.deepEqual(
        sorted(bodySchema.required ?? []),
        sorted(
          bodyParameters
            .filter((parameter) => !parameter.questionToken)
            .map((parameter) => parameterName(parameter, apiSource)),
        ),
        `${operationId} required request fields drifted`,
      );
      for (const parameter of bodyParameters) {
        const name = parameterName(parameter, apiSource);
        assert.deepEqual(
          schemaDescriptor(bodySchema.properties[name]),
          typeDescriptor(parameter.type, apiSource),
          `${operationId}.${name} request type drifted`,
        );
      }
    }

    const result = promiseResult(method, apiSource);
    const success = record.operation.responses[String(expected.success)];
    const responseSchema = success.content?.['application/json']?.schema;
    if (result.kind === 'void') {
      assert.equal(responseSchema, undefined, `${operationId} Promise<void> must not return JSON`);
    } else {
      assert.deepEqual(schemaDescriptor(responseSchema), result, `${operationId} response schema drifted`);
    }
  }
});

test('frontend schema names, fields, optionality, enums, and union variants match OpenAPI', () => {
  const openapi = parseOpenApi();
  const schemas = openapi.components?.schemas ?? {};
  const { declarations } = collectTypeScriptContract();
  const typeScriptSchemaNames = [...declarations.keys()].filter((name) => name !== 'ApiClient');
  const openApiSchemaNames = Object.keys(schemas).filter((name) => name !== 'Error');
  assert.deepEqual(sorted(openApiSchemaNames), sorted(typeScriptSchemaNames));

  for (const name of typeScriptSchemaNames) {
    const { declaration, source } = declarations.get(name);
    const schema = schemas[name];
    assert.ok(schema, `OpenAPI is missing schema ${name}`);

    if (ts.isInterfaceDeclaration(declaration)) {
      const typeScriptShape = ownInterfaceShape(declaration, source);
      const openApiShape = ownSchemaShape(schema);
      assertPropertyShape(
        name,
        typeScriptShape,
        new Map(
          Object.keys(openApiShape.properties).map((property) => [property, openApiShape.required.has(property)]),
        ),
      );
      continue;
    }

    const enumValues = stringEnumValues(declaration.type);
    if (enumValues) {
      assert.deepEqual(schema.enum, enumValues, `${name} enum values differ between TypeScript and OpenAPI`);
      continue;
    }

    const typeScriptVariants = discriminatedTypeScriptVariants(declaration.type);
    const openApiVariants = discriminatedOpenApiVariants(schema);
    if (!typeScriptVariants && !openApiVariants) continue;
    assert.ok(typeScriptVariants && openApiVariants, `${name} must be a discriminated union in both contracts`);
    assert.deepEqual(
      sorted(typeScriptVariants.map((variant) => variant.key)),
      sorted(openApiVariants.map((variant) => variant.key)),
    );
    for (const typeScriptVariant of typeScriptVariants) {
      const openApiVariant = openApiVariants.find((variant) => variant.key === typeScriptVariant.key);
      assertPropertyShape(`${name}.${typeScriptVariant.key}`, typeScriptVariant.properties, openApiVariant.properties);
    }
  }
});

test('all local OpenAPI references resolve', () => {
  const openapi = parseOpenApi();
  const visit = (value) => {
    if (Array.isArray(value)) {
      value.forEach(visit);
      return;
    }
    if (!value || typeof value !== 'object') return;
    if (value.$ref) assert.ok(resolveLocalRef(openapi, value), `unresolved OpenAPI reference: ${value.$ref}`);
    Object.values(value).forEach(visit);
  };
  visit(openapi);
});

test('security-sensitive lifecycle and ledger constraints are frozen', () => {
  const openapi = parseOpenApi();
  const operations = collectOperations(openapi);

  const tripStatusBody = operations.get('setTripStatus').operation.requestBody.content['application/json'].schema;
  assert.equal(tripStatusBody.additionalProperties, false);
  assert.equal(localRefName(tripStatusBody.properties.status), 'TripStatus');

  const candidateStatusBody =
    operations.get('setCandidateStatus').operation.requestBody.content['application/json'].schema;
  assert.equal(candidateStatusBody.additionalProperties, false);
  assert.equal(localRefName(candidateStatusBody.properties.status), 'CandidateDisposition');
  assert.deepEqual(openapi.components.schemas.CandidateDisposition.enum, ['shortlisted', 'rejected']);

  const expensePatch = openapi.components.schemas.ExpensePatch;
  assert.equal(expensePatch.additionalProperties, false);
  assert.equal(expensePatch.minProperties, 1);
  assert.ok(!Object.hasOwn(expensePatch.properties, 'fxRateToBase'), 'clients must not choose frozen exchange rates');
  assert.deepEqual(expensePatch.properties.linkedStopId.type, ['string', 'null']);

  const poll = openapi.components.schemas.Poll;
  assert.ok(!poll.required.includes('decidedAt'), 'legacy poll records may omit decidedAt');
  assert.deepEqual(poll.properties.decidedAt.type, ['string', 'null']);
  assert.equal(poll.properties.decidedAt.format, 'date-time');
});

test('currently implemented Rust application routes are represented by OpenAPI', () => {
  const openapi = parseOpenApi();
  const rustRouter = fs.readFileSync(rustRouterPath, 'utf8');
  const routes = [...rustRouter.matchAll(/\.route\(\s*"([^"]+)"\s*,\s*(get|post|put|patch|delete)\s*\(/g)].map(
    ([, route, method]) => ({ route, method }),
  );
  assert.ok(routes.length > 0, 'no Rust routes found; update the contract test if router composition changes');
  for (const { route, method } of routes) {
    if (route === '/healthz') continue; // Operational endpoint, deliberately outside ApiClient.
    assert.ok(openapi.paths?.[route]?.[method], `Rust ${method.toUpperCase()} ${route} is missing from OpenAPI`);
  }
});
