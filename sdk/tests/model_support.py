"""ctypes test adapter over generated Rust model descriptors, with owned input storage."""
import ctypes as c
import re
from models_generated import DEFINITIONS


def camel(name):
    first, *rest = name.split('_')
    return first + ''.join(part.title() for part in rest)


def kebab(name):
    return re.sub(r'(?<=[a-z0-9])(?=[A-Z])|(?<=[A-Z])(?=[A-Z][a-z])', '-', name).lower()


TYPES = {name: getattr(c, native) for name, native in {
    'bool': 'c_uint32', 'u8': 'c_uint8', 'u16': 'c_uint16', 'u32': 'c_uint32',
    'u64': 'c_uint64', 'usize': 'c_size_t', 'i8': 'c_int8', 'i16': 'c_int16',
    'i32': 'c_int32', 'i64': 'c_int64', 'isize': 'c_ssize_t', 'f32': 'c_float', 'f64': 'c_double'
}.items()}
PRIMITIVES = set(TYPES)


def structure(name, fields):
    return type(name, (c.Structure,), {'_fields_': fields})


TYPES['Text'] = structure('Text', [('data', c.c_void_p), ('size', c.c_size_t)])
TYPES['TextEntry'] = structure('TextEntry', [('key', TYPES['Text']), ('value', TYPES['Text'])])
for name, spec in DEFINITIONS.items():
    kind = spec[0]
    if kind == 'struct':
        native = structure(name, [(field, TYPES[t]) for field, t in spec[1]])
    elif kind == 'optional':
        native = structure(name, [('has_value', c.c_uint32), ('value', TYPES[spec[1]])])
    elif kind == 'slice':
        native = structure(name, [('data', c.c_void_p), ('size', c.c_size_t)])
    elif all(not fields for _, fields in spec[2]):
        native = c.c_uint32
    else:
        native = structure(name, [('kind', c.c_uint32)] + [
            (variant, structure(name + variant, [(f, TYPES[t]) for f, t in fields]))
            for variant, fields in spec[2] if fields])
    TYPES[name] = native


def read(name, value):
    if name in PRIMITIVES:
        return bool(value) if name == 'bool' else value
    if name == 'Text':
        return c.string_at(value.data, value.size).decode('utf-8') if value.size else ''
    if name == 'TextEntry':
        return (read('Text', value.key), read('Text', value.value))
    spec = DEFINITIONS[name]
    kind = spec[0]
    if kind == 'struct':
        return {camel(f): read(t, getattr(value, f)) for f, t in spec[1]}
    if kind == 'optional':
        return read(spec[1], value.value) if value.has_value else None
    if kind == 'slice':
        items = c.cast(value.data, c.POINTER(TYPES[spec[1]]))
        result = [read(spec[1], items[i]) for i in range(value.size)]
        return dict(result) if spec[1] == 'TextEntry' else result
    if isinstance(value, int):
        return kebab(spec[2][value - spec[1]][0])
    variant, fields = spec[2][value.kind - spec[1]]
    result = {'kind': variant}
    if fields:
        result.update({camel(f): read(t, getattr(getattr(value, variant), f)) for f, t in fields})
    return result


def read_pointer(name, pointer):
    if not pointer:
        raise ValueError('Missing typed result: ' + name)
    return read(name, c.cast(pointer, c.POINTER(TYPES[name])).contents)


def write(name, value, owners):
    if name in PRIMITIVES:
        return int(value) if name == 'bool' else value
    if name == 'Text':
        data = value.encode('utf-8')
        buffer = c.create_string_buffer(data)
        owners.append(buffer)
        return TYPES[name](c.cast(buffer, c.c_void_p), len(data))
    if name == 'TextEntry':
        return TYPES[name](write('Text', value[0], owners), write('Text', value[1], owners))
    spec = DEFINITIONS[name]
    kind = spec[0]
    if kind == 'struct':
        return TYPES[name](**{f: write(t, value[camel(f)], owners) for f, t in spec[1]})
    if kind == 'optional':
        return TYPES[name]() if value is None else TYPES[name](1, write(spec[1], value, owners))
    if kind == 'slice':
        values = list(value.items()) if spec[1] == 'TextEntry' else value
        array = (TYPES[spec[1]] * len(values))(*(write(spec[1], item, owners) for item in values))
        owners.append(array)
        return TYPES[name](c.cast(array, c.c_void_p), len(values))
    if isinstance(value, str):
        return next(i for i, (variant, _) in enumerate(spec[2], spec[1]) if kebab(variant) == value)
    index, (variant, fields) = next((i, v) for i, v in enumerate(spec[2], spec[1]) if v[0] == value['kind'])
    result = TYPES[name](kind=index)
    for field, t in fields:
        setattr(getattr(result, variant), field, write(t, value[camel(field)], owners))
    return result
